//! Cover cache: asks the backend for each image one time and keeps the uploaded textures. It
//! removes the least recently used textures.

use std::collections::HashMap;

use egui::load::SizedTexture;
use egui::{Color32, ColorImage, Context, TextureFilter, TextureHandle, TextureOptions};

use crate::backend::DecodedImage;

/// The size requested for list thumbnails (shown at approximately 44 points).
pub const THUMB_PX: u32 = 96;

const MAX_TEXTURES: usize = 400;

const RETRY_FAILED_AFTER: f64 = 30.0;

/// A cover ready to paint. It only refers to the texture, which the cache keeps, so a copy has no
/// cost. A clone of a [`TextureHandle`] (and its drop) locks the texture manager of egui.
#[derive(Clone, Copy)]
pub struct Cover {
    pub texture: SizedTexture,
    /// The UI time when it loaded, to fade it in.
    pub loaded_at: f64,
    pub accent: Option<Color32>,
}

enum Slot {
    Pending,
    Ready {
        cover: Cover,
        /// Keeps the texture while it is in the cache.
        _handle: TextureHandle,
        last_used: u64,
    },
    Failed {
        at: f64,
    },
}

#[derive(Default)]
pub struct TextureCache {
    /// URL → size in pixels → slot. It is nested, so that the views, which ask for each cover in
    /// each frame, find them without an allocation.
    slots: HashMap<String, HashMap<u32, Slot>>,
    requests: Vec<(String, u32)>,
    frame: u64,
}

impl TextureCache {
    pub fn begin_frame(&mut self) {
        self.frame += 1;
    }

    /// The images to ask the backend for, collected during the draw.
    pub fn take_requests(&mut self) -> Vec<(String, u32)> {
        std::mem::take(&mut self.requests)
    }

    /// Returns the cover if it is loaded. If not, it requests it.
    pub fn get(&mut self, url: &str, px: u32, now: f64) -> Option<Cover> {
        match self.slots.get_mut(url).and_then(|sizes| sizes.get_mut(&px)) {
            Some(Slot::Ready {
                cover, last_used, ..
            }) => {
                *last_used = self.frame;
                return Some(*cover);
            }
            Some(Slot::Pending) => return None,
            Some(Slot::Failed { at }) if now - *at < RETRY_FAILED_AFTER => return None,
            Some(Slot::Failed { .. }) | None => {}
        }
        self.set(url.to_owned(), px, Slot::Pending);
        self.requests.push((url.to_owned(), px));
        None
    }

    pub fn insert(&mut self, ctx: &Context, url: String, px: u32, image: &DecodedImage, now: f64) {
        let size = [image.width as usize, image.height as usize];
        let color_image = ColorImage::from_rgba_unmultiplied(size, &image.rgba);
        let options = TextureOptions::LINEAR.with_mipmap_mode(Some(TextureFilter::Linear));
        let handle = ctx.load_texture(format!("{url}@{px}"), color_image, options);
        let cover = Cover {
            texture: SizedTexture::from_handle(&handle),
            loaded_at: now,
            accent: image.accent.map(|[r, g, b]| Color32::from_rgb(r, g, b)),
        };
        let slot = Slot::Ready {
            cover,
            _handle: handle,
            last_used: self.frame,
        };
        self.set(url, px, slot);
        self.evict();
    }

    pub fn mark_failed(&mut self, url: String, px: u32, now: f64) {
        self.set(url, px, Slot::Failed { at: now });
    }

    fn set(&mut self, url: String, px: u32, slot: Slot) {
        self.slots.entry(url).or_default().insert(px, slot);
    }

    /// Removes the least recently used textures after [`MAX_TEXTURES`].
    fn evict(&mut self) {
        let mut ready: Vec<(u64, &String, u32)> = self
            .slots
            .iter()
            .flat_map(|(url, sizes)| {
                sizes.iter().filter_map(move |(px, slot)| match slot {
                    Slot::Ready { last_used, .. } => Some((*last_used, url, *px)),
                    _ => None,
                })
            })
            .collect();
        if ready.len() <= MAX_TEXTURES {
            return;
        }
        ready.sort_unstable_by_key(|(last_used, ..)| *last_used);
        let excess = ready.len() - MAX_TEXTURES;
        let evicted: Vec<(String, u32)> = ready[..excess]
            .iter()
            .map(|(_, url, px)| ((*url).clone(), *px))
            .collect();
        for (url, px) in evicted {
            if let Some(sizes) = self.slots.get_mut(&url) {
                sizes.remove(&px);
                if sizes.is_empty() {
                    self.slots.remove(&url);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image() -> DecodedImage {
        DecodedImage {
            width: 1,
            height: 1,
            rgba: vec![255; 4],
            accent: Some([30, 215, 96]),
        }
    }

    fn requested(cache: &mut TextureCache) -> Vec<String> {
        cache
            .take_requests()
            .into_iter()
            .map(|(url, px)| format!("{url}@{px}"))
            .collect()
    }

    #[test]
    fn each_cover_is_asked_for_once_until_it_arrives() {
        let ctx = Context::default();
        let mut cache = TextureCache::default();
        assert!(cache.get("opera", THUMB_PX, 0.0).is_none());
        assert!(cache.get("opera", THUMB_PX, 0.1).is_none());
        assert_eq!(requested(&mut cache), ["opera@96"]);

        cache.insert(&ctx, "opera".into(), THUMB_PX, &image(), 1.0);
        let cover = cache.get("opera", THUMB_PX, 2.0).unwrap();
        assert_eq!(cover.accent, Some(Color32::from_rgb(30, 215, 96)));
        assert!((cover.loaded_at - 1.0).abs() < f64::EPSILON);
        assert!(requested(&mut cache).is_empty());
        // A different size is a different texture.
        assert!(cache.get("opera", 192, 2.0).is_none());
        assert_eq!(requested(&mut cache), ["opera@192"]);
    }

    #[test]
    fn a_cover_that_failed_is_asked_for_again_after_a_while() {
        let mut cache = TextureCache::default();
        cache.get("opera", THUMB_PX, 0.0);
        cache.take_requests();
        cache.mark_failed("opera".into(), THUMB_PX, 1.0);
        assert!(cache.get("opera", THUMB_PX, 2.0).is_none());
        assert!(requested(&mut cache).is_empty());
        assert!(
            cache
                .get("opera", THUMB_PX, 1.0 + RETRY_FAILED_AFTER)
                .is_none()
        );
        assert_eq!(requested(&mut cache), ["opera@96"]);
    }

    #[test]
    fn the_covers_used_least_recently_go_first() {
        let ctx = Context::default();
        let mut cache = TextureCache::default();
        for i in 0..=MAX_TEXTURES {
            cache.begin_frame();
            if i == MAX_TEXTURES {
                // The first texture is still in use: the second texture is the oldest.
                assert!(cache.get("0", THUMB_PX, 0.0).is_some());
            }
            cache.insert(&ctx, i.to_string(), THUMB_PX, &image(), 0.0);
        }
        assert!(cache.get("0", THUMB_PX, 0.0).is_some());
        assert!(
            cache
                .get(&MAX_TEXTURES.to_string(), THUMB_PX, 0.0)
                .is_some()
        );
        assert!(cache.get("1", THUMB_PX, 0.0).is_none(), "evicted");
        assert_eq!(requested(&mut cache), ["1@96"]);
    }
}
