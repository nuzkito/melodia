//! Back and forward through the contents of the middle column: the playlists, albums and artists
//! that the user opened, the searches (with the last text typed in each) and the guide that shows
//! when none of them is open. As in a web browser, a new place removes the places ahead.
//!
//! The history does not get a message for each change. [`History::visit`] gets the contents of
//! the screen in each frame. Thus each method that changes the contents counts (a click, Escape,
//! «Clear search», a playlist that disappears).

/// The contents of the middle column.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Place {
    /// No search, playlist, album or artist: the guide.
    Home,
    /// The tracks of the playlist with this id.
    Playlist(String),
    /// The album with this id, and its name.
    Album { id: String, name: String },
    /// The artist with this id, and their name.
    Artist { id: String, name: String },
    /// The results of a search, with the text in the search box.
    Search(String),
}

/// The places that the history keeps. The oldest places go first.
const MAX_PLACES: usize = 100;

#[derive(Debug)]
pub struct History {
    places: Vec<Place>,
    /// The index of the place that shows.
    index: usize,
}

impl Default for History {
    fn default() -> Self {
        Self {
            places: vec![Place::Home],
            index: 0,
        }
    }
}

impl History {
    #[cfg(test)]
    fn current(&self) -> &Place {
        &self.places[self.index]
    }

    /// `place` is on the screen. A search that the user types stays one place, with its latest
    /// text (an album or an artist, with its latest name). Any other new place replaces the places
    /// ahead.
    pub fn visit(&mut self, place: Place) {
        let current = &mut self.places[self.index];
        if *current == place {
            return;
        }
        match (&mut *current, &place) {
            (Place::Search(text), Place::Search(new)) => {
                text.clone_from(new);
                return;
            }
            // Its name, after it loads, can be different from its name when it opened.
            (
                Place::Album { id, name },
                Place::Album {
                    id: new_id,
                    name: new,
                },
            ) if id == new_id => {
                name.clone_from(new);
                return;
            }
            (
                Place::Artist { id, name },
                Place::Artist {
                    id: new_id,
                    name: new,
                },
            ) if id == new_id => {
                name.clone_from(new);
                return;
            }
            _ => {}
        }
        self.places.truncate(self.index + 1);
        self.places.push(place);
        if self.places.len() > MAX_PLACES {
            self.places.remove(0);
        }
        self.index = self.places.len() - 1;
    }

    pub fn can_go_back(&self) -> bool {
        self.index > 0
    }

    pub fn can_go_forward(&self) -> bool {
        self.index + 1 < self.places.len()
    }

    /// Moves one place back (or forward). It skips places that cannot show now (`available`, for
    /// example a deleted playlist). Returns the place to show, or `None` if there is none and
    /// nothing changes.
    pub fn go(&mut self, forward: bool, available: impl Fn(&Place) -> bool) -> Option<Place> {
        let mut index = self.index;
        loop {
            index = if forward {
                Some(index + 1).filter(|&i| i < self.places.len())?
            } else {
                index.checked_sub(1)?
            };
            if available(&self.places[index]) {
                self.index = index;
                return Some(self.places[index].clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn playlist(id: &str) -> Place {
        Place::Playlist(id.into())
    }

    fn search(text: &str) -> Place {
        Place::Search(text.into())
    }

    fn back(history: &mut History) -> Option<Place> {
        history.go(false, |_| true)
    }

    fn forward(history: &mut History) -> Option<Place> {
        history.go(true, |_| true)
    }

    #[test]
    fn back_goes_through_the_playlists_in_the_order_they_were_opened() {
        let mut history = History::default();
        for id in ["a", "c", "b"] {
            history.visit(playlist(id));
        }
        assert_eq!(back(&mut history), Some(playlist("c")));
        assert_eq!(back(&mut history), Some(playlist("a")));
        assert_eq!(back(&mut history), Some(Place::Home));
        assert_eq!(back(&mut history), None);
        assert_eq!(history.current(), &Place::Home);

        assert_eq!(forward(&mut history), Some(playlist("a")));
        assert_eq!(forward(&mut history), Some(playlist("c")));
        assert_eq!(forward(&mut history), Some(playlist("b")));
        assert_eq!(forward(&mut history), None);
    }

    #[test]
    fn a_search_is_one_place_with_its_last_text() {
        let mut history = History::default();
        history.visit(playlist("a"));
        for text in ["q", "qu", "queen"] {
            history.visit(search(text));
        }
        history.visit(playlist("b"));
        assert_eq!(back(&mut history), Some(search("queen")));
        assert_eq!(back(&mut history), Some(playlist("a")));
    }

    #[test]
    fn going_somewhere_new_drops_the_places_ahead() {
        let mut history = History::default();
        history.visit(playlist("a"));
        history.visit(playlist("b"));
        back(&mut history);
        history.visit(playlist("c"));
        assert!(!history.can_go_forward());
        assert_eq!(back(&mut history), Some(playlist("a")));
    }

    #[test]
    fn editing_a_search_gone_back_to_keeps_the_places_ahead() {
        let mut history = History::default();
        history.visit(search("queen"));
        history.visit(playlist("a"));
        back(&mut history);
        history.visit(search("queen live"));
        assert_eq!(forward(&mut history), Some(playlist("a")));
        assert_eq!(back(&mut history), Some(search("queen live")));
    }

    #[test]
    fn places_that_are_gone_are_skipped() {
        let mut history = History::default();
        history.visit(playlist("a"));
        history.visit(playlist("gone"));
        history.visit(playlist("b"));
        let available = |place: &Place| *place != playlist("gone");
        assert_eq!(history.go(false, available), Some(playlist("a")));
        assert_eq!(history.go(true, available), Some(playlist("b")));
        // When there is no place to go to, it stays at the same place.
        let mut history = History::default();
        history.visit(playlist("gone"));
        assert_eq!(history.go(false, |_| false), None);
        assert_eq!(history.current(), &playlist("gone"));
    }

    #[test]
    fn albums_are_places_too() {
        let mut history = History::default();
        let album = Place::Album {
            id: "opera".into(),
            name: "A Night at the Opera".into(),
        };
        history.visit(playlist("a"));
        history.visit(Place::Album {
            id: "opera".into(),
            name: "Opera".into(),
        });
        history.visit(album.clone());
        history.visit(search("queen"));
        assert_eq!(back(&mut history), Some(album));
        assert_eq!(back(&mut history), Some(playlist("a")));
    }

    #[test]
    fn artists_are_places_too() {
        let mut history = History::default();
        let queen = Place::Artist {
            id: "queen".into(),
            name: "Queen".into(),
        };
        history.visit(Place::Artist {
            id: "queen".into(),
            name: "Queen (loading)".into(),
        });
        history.visit(queen.clone());
        history.visit(playlist("a"));
        assert_eq!(back(&mut history), Some(queen));
        assert_eq!(back(&mut history), Some(Place::Home));
    }

    #[test]
    fn the_oldest_places_are_forgotten() {
        let mut history = History::default();
        for i in 0..MAX_PLACES + 10 {
            history.visit(playlist(&i.to_string()));
        }
        let mut steps = 0;
        while back(&mut history).is_some() {
            steps += 1;
        }
        assert_eq!(steps, MAX_PLACES - 1);
        assert_eq!(history.current(), &playlist("10"));
    }
}
