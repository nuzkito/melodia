//! The user's folders and order of their playlists. Spotify's Web API cannot see folders or change
//! the order of the playlists of a library. Thus the app keeps the library that the user arranged
//! in the sidebar, in `settings.json` for each account. It applies it each time the playlists load.

use std::path::PathBuf;

use crate::config;
use crate::model::{Library, LibraryEdit, Playlist};

pub struct PlaylistLibrary {
    /// The save location. `None` keeps it only in memory.
    config_dir: Option<PathBuf>,
    /// The Spotify account of the playlists that the library last arranged.
    user: Option<String>,
    library: Library,
    /// The user arranged it (here or before). If not, it follows Spotify's order each time the
    /// playlists load.
    custom: bool,
    /// The number of edits applied (see [`crate::backend::Event::Library`]).
    edits: u64,
}

impl PlaylistLibrary {
    pub fn new(config_dir: Option<PathBuf>) -> Self {
        Self {
            config_dir,
            user: None,
            library: Library::default(),
            custom: false,
            edits: 0,
        }
    }

    /// A library of `user` that the user already arranged, only in memory. The demo starts with it.
    pub fn in_memory(user: &str, library: Library) -> Self {
        Self {
            config_dir: None,
            user: Some(user.to_owned()),
            library,
            custom: true,
            edits: 0,
        }
    }

    /// Changes the library of `user` to agree with their playlists, as Spotify lists them (see
    /// [`Library::arrange`]), and returns them in its order. New playlists get their position in
    /// it, and deleted playlists leave it. The saved library changes in the same way.
    pub fn arrange(&mut self, user: &str, playlists: Vec<Playlist>) -> Vec<Playlist> {
        if self.user.as_deref() != Some(user) {
            self.user = Some(user.to_owned());
            let saved = self
                .config_dir
                .as_deref()
                .and_then(|dir| config::playlist_library(dir, user));
            self.custom = saved.is_some();
            self.library = saved.unwrap_or_default();
        }
        if !self.custom {
            self.library = Library::default();
        }
        let before = self.library.clone();
        let arranged = self.library.arrange(playlists);
        if self.custom && self.library != before {
            self.save();
        }
        arranged
    }

    /// The library as last arranged or edited.
    pub fn library(&self) -> &Library {
        &self.library
    }

    pub fn edits(&self) -> u64 {
        self.edits
    }

    /// Applies an edit from the sidebar to the playlists that loaded last. Returns true if it must
    /// be saved (see [`Self::save`]).
    #[must_use]
    pub fn apply(&mut self, edit: &LibraryEdit) -> bool {
        self.edits += 1;
        if self.user.is_none() || !self.library.apply(edit) {
            return false;
        }
        self.custom = true;
        true
    }

    /// The app just created the playlist `id` in the folder `folder`: it goes there. Returns true
    /// if it must be saved.
    #[must_use]
    pub fn place_new(&mut self, id: &str, folder: &str) -> bool {
        if self.user.is_none() || self.library.folder(folder).is_none() {
            return false;
        }
        self.library.place_new(id, folder);
        self.custom = true;
        true
    }

    /// Writes the current library to the disk (does nothing when it is only in memory).
    pub fn save(&self) {
        let (Some(dir), Some(user)) = (&self.config_dir, &self.user) else {
            return;
        };
        if let Err(error) = config::save_playlist_library(dir, user, &self.library) {
            log::warn!("Could not save the folders of the playlists: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{LibraryItem, LibraryKey};

    fn playlists(ids: &[&str]) -> Vec<Playlist> {
        ids.iter()
            .map(|id| Playlist {
                id: (*id).to_owned(),
                name: id.to_uppercase(),
                owner: String::new(),
                total: 0,
                cover: None,
                editable: true,
                snapshot: None,
            })
            .collect()
    }

    fn ids(playlists: &[Playlist]) -> Vec<&str> {
        playlists.iter().map(|p| p.id.as_str()).collect()
    }

    fn to_top(id: &str) -> LibraryEdit {
        LibraryEdit::Move {
            item: LibraryKey::Playlist(id.to_owned()),
            to: None,
            index: Some(0),
        }
    }

    #[test]
    fn the_library_survives_a_restart_and_follows_the_playlists() {
        let dir = tempfile::tempdir().unwrap();
        let mut library = PlaylistLibrary::new(Some(dir.path().to_owned()));
        let listed = library.arrange("alice", playlists(&["a", "b", "c"]));
        assert_eq!(ids(&listed), ["a", "b", "c"], "Spotify's at first");
        assert_eq!(
            config::playlist_library(dir.path(), "alice"),
            None,
            "nothing saved until arranged"
        );
        // Until then, the library follows Spotify's order.
        let listed = library.arrange("alice", playlists(&["c", "a", "b"]));
        assert_eq!(ids(&listed), ["c", "a", "b"]);

        assert!(library.apply(&LibraryEdit::CreateFolder {
            id: "folder-1".into(),
            name: "Festivals".into(),
            parent: None,
        }));
        assert!(library.apply(&LibraryEdit::Move {
            item: LibraryKey::Playlist("b".into()),
            to: Some("folder-1".into()),
            index: None,
        }));
        library.save();
        assert!(!library.apply(&to_top("folder-gone")), "nothing to change");
        assert_eq!(library.edits(), 3);

        // The next time the app opens, with a new playlist and a deleted playlist.
        let mut library = PlaylistLibrary::new(Some(dir.path().to_owned()));
        let listed = library.arrange("alice", playlists(&["new", "a", "b"]));
        assert_eq!(ids(&listed), ["new", "b", "a"]);
        let saved = config::playlist_library(dir.path(), "alice").unwrap();
        assert_eq!(saved.playlist_ids(), ["new", "b", "a"]);
        assert!(matches!(&saved.items()[1], LibraryItem::Folder(f) if f.name == "Festivals"));

        // A different account has its own library.
        let listed = library.arrange("bob", playlists(&["x", "y"]));
        assert_eq!(ids(&listed), ["x", "y"]);
        assert!(library.apply(&to_top("y")));
        library.save();
        let listed = library.arrange("alice", playlists(&["b", "a"]));
        assert_eq!(ids(&listed), ["b", "a"]);
        assert_eq!(
            config::playlist_library(dir.path(), "bob").map(|l| l.playlist_ids()),
            Some(vec!["y".to_owned(), "x".to_owned()])
        );
    }

    #[test]
    fn nothing_is_arranged_before_the_playlists_are_listed() {
        let mut library = PlaylistLibrary::new(None);
        assert!(!library.apply(&to_top("b")));
        library.arrange("demo", playlists(&["a", "b"]));
        assert!(library.apply(&to_top("b")));
        let listed = library.arrange("demo", playlists(&["a", "b"]));
        assert_eq!(ids(&listed), ["b", "a"], "kept in memory");
    }

    #[test]
    fn a_playlist_created_in_a_folder_is_listed_there() {
        let mut library = PlaylistLibrary::new(None);
        library.arrange("demo", playlists(&["a"]));
        assert!(library.apply(&LibraryEdit::CreateFolder {
            id: "folder-1".into(),
            name: "Viajes".into(),
            parent: None,
        }));
        assert!(library.place_new("new", "folder-1"));
        assert!(!library.place_new("other", "folder-gone"));
        // "other" comes first in Spotify's order, so it goes to the top.
        let listed = library.arrange("demo", playlists(&["other", "new", "a"]));
        assert_eq!(ids(&listed), ["other", "new", "a"]);
        assert_eq!(
            library.library().level(Some("folder-1")),
            Some(&[LibraryItem::Playlist("new".into())][..])
        );
    }
}
