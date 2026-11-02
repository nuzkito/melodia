//! The folders of the playlists. Spotify's Web API cannot see or change the folders of a library
//! (or its order), so the app keeps its own. This is a tree of folders and playlist IDs, saved for
//! each account in `settings.json`. The sidebar shows it one level at a time.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use super::{Playlist, arrange_playlists};
use crate::util::plural;

/// An entry of a level of the library: a playlist (its ID) or a folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LibraryItem {
    Playlist(String),
    Folder(Folder),
}

impl LibraryItem {
    pub fn key(&self) -> LibraryKey {
        match self {
            Self::Playlist(id) => LibraryKey::Playlist(id.clone()),
            Self::Folder(folder) => LibraryKey::Folder(folder.id.clone()),
        }
    }

    fn is(&self, key: &LibraryKey) -> bool {
        match (self, key) {
            (Self::Playlist(id), LibraryKey::Playlist(wanted)) => id == wanted,
            (Self::Folder(folder), LibraryKey::Folder(wanted)) => &folder.id == wanted,
            _ => false,
        }
    }
}

/// A folder of playlists (and other folders), made in the app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Folder {
    /// The app makes it («folder-1»…). It is unique in the library.
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub items: Vec<LibraryItem>,
}

impl Folder {
    /// The first playlist in it, also in its folders (its cover gives the folder its color).
    pub fn first_playlist(&self) -> Option<&str> {
        first_playlist(&self.items)
    }

    /// The number of playlists in it, with the playlists of its folders.
    pub fn playlist_count(&self) -> usize {
        self.items
            .iter()
            .map(|item| match item {
                LibraryItem::Playlist(_) => 1,
                LibraryItem::Folder(folder) => folder.playlist_count(),
            })
            .sum()
    }

    /// True if the playlist `id` is in it, at some level.
    pub fn holds_playlist(&self, id: &str) -> bool {
        self.items.iter().any(|item| match item {
            LibraryItem::Playlist(other) => other == id,
            LibraryItem::Folder(folder) => folder.holds_playlist(id),
        })
    }

    /// "Folder · 3 playlists", as the sidebar shows it under its name.
    pub fn count_line(&self) -> String {
        match self.playlist_count() {
            0 if self.items.is_empty() => "Folder · Empty".to_owned(),
            count => format!("Folder · {}", plural(count, "playlist", "playlists")),
        }
    }
}

fn first_playlist(items: &[LibraryItem]) -> Option<&str> {
    items.iter().find_map(|item| match item {
        LibraryItem::Playlist(id) => Some(id.as_str()),
        LibraryItem::Folder(folder) => folder.first_playlist(),
    })
}

/// An entry of the library, by identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LibraryKey {
    Playlist(String),
    Folder(String),
}

/// A change to the folders that the user makes. The UI applies it immediately with
/// [`Library::apply`], and the backend does the same before it saves it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LibraryEdit {
    /// Creates the empty folder `id` with the name `name` at the top of the folder `parent`
    /// (`None`: the first level).
    CreateFolder {
        id: String,
        name: String,
        parent: Option<String>,
    },
    RenameFolder {
        id: String,
        name: String,
    },
    /// Removes the folder `id`. Its contents go to its position.
    DeleteFolder {
        id: String,
    },
    /// Moves `item` into the folder `to` (`None`: the first level), to `index` (as `Vec::remove`,
    /// then `Vec::insert`), or to the end with `None`.
    Move {
        item: LibraryKey,
        to: Option<String>,
        index: Option<usize>,
    },
}

/// The user's playlists and folders, as a tree. Each playlist that Spotify lists is in it exactly
/// one time (see [`Library::arrange`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Library {
    items: Vec<LibraryItem>,
}

impl Library {
    pub fn new(items: Vec<LibraryItem>) -> Self {
        Self { items }
    }

    /// A library without folders, with the playlists in this order (as the app saved it before it
    /// had folders).
    pub fn from_order(ids: &[String]) -> Self {
        Self::new(ids.iter().cloned().map(LibraryItem::Playlist).collect())
    }

    /// The first level.
    pub fn items(&self) -> &[LibraryItem] {
        &self.items
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The contents of the folder `folder` (`None`: the first level), if it exists.
    pub fn level(&self, folder: Option<&str>) -> Option<&[LibraryItem]> {
        match folder {
            None => Some(&self.items),
            Some(id) => self.folder(id).map(|f| f.items.as_slice()),
        }
    }

    pub fn folder(&self, id: &str) -> Option<&Folder> {
        find_folder(&self.items, id)
    }

    /// The folders on the path to the folder `id`, from the first level, with `id`. Empty if it
    /// does not exist.
    pub fn path_to(&self, id: &str) -> Vec<&Folder> {
        let mut path = Vec::new();
        path_to(&self.items, id, &mut path);
        path
    }

    /// The folder that holds `key` (`Some(None)`: the first level), if it is in the library.
    pub fn parent_of(&self, key: &LibraryKey) -> Option<Option<&str>> {
        if self.items.iter().any(|item| item.is(key)) {
            return Some(None);
        }
        parent_of(&self.items, key).map(Some)
    }

    /// The IDs of the playlists, from top to bottom. Each folder adds its playlists at its
    /// position.
    pub fn playlist_ids(&self) -> Vec<String> {
        let mut ids = Vec::new();
        collect_playlists(&self.items, &mut ids);
        ids
    }

    /// An ID for a new folder that no other folder has.
    pub fn next_folder_id(&self) -> String {
        let mut ids = Vec::new();
        collect_folders(&self.items, &mut ids);
        let highest = ids
            .iter()
            .filter_map(|id| id.strip_prefix("folder-")?.parse::<u64>().ok())
            .max()
            .unwrap_or(0);
        format!("folder-{}", highest + 1)
    }

    /// Changes the library to agree with `playlists`, as Spotify lists them, and returns them in
    /// its order (each folder adds its playlists at its position). The function removes deleted
    /// playlists. A new playlist goes immediately after the playlist before it in Spotify's order,
    /// in the same folder. If there is no playlist before it, it goes at the top of the first level
    /// (as with [`arrange_playlists`]). Folders stay, also when empty.
    pub fn arrange(&mut self, playlists: Vec<Playlist>) -> Vec<Playlist> {
        let listed: HashSet<String> = playlists.iter().map(|p| p.id.clone()).collect();
        let mut seen = HashSet::new();
        retain_in(&mut self.items, &listed, &mut seen, &mut HashSet::new());

        let arranged = arrange_playlists(playlists, &self.playlist_ids());
        let mut previous: Option<&str> = None;
        for playlist in &arranged {
            if !seen.contains(playlist.id.as_str()) {
                let item = LibraryItem::Playlist(playlist.id.clone());
                match previous {
                    Some(previous) => {
                        let key = LibraryKey::Playlist(previous.to_owned());
                        let (items, index) = locate_mut(&mut self.items, &key)
                            .expect("the previous playlist is in the library");
                        items.insert(index + 1, item);
                    }
                    None => self.items.insert(0, item),
                }
            }
            previous = Some(&playlist.id);
        }

        // In the order of the library, which `arrange_playlists` already gave them. Sort them
        // again, because a folder can change that order.
        let ranks: HashMap<String, usize> = self
            .playlist_ids()
            .into_iter()
            .enumerate()
            .map(|(rank, id)| (id, rank))
            .collect();
        let mut arranged = arranged;
        arranged.sort_by_key(|p| ranks.get(&p.id).copied().unwrap_or(usize::MAX));
        arranged
    }

    /// Puts the playlist `id`, which is not in the library yet (it is new), at the top of the
    /// folder `folder`, if it exists.
    pub fn place_new(&mut self, id: &str, folder: &str) {
        let key = LibraryKey::Playlist(id.to_owned());
        if self.parent_of(&key).is_some() {
            return;
        }
        if let Some(folder) = find_folder_mut(&mut self.items, folder) {
            folder.items.insert(0, LibraryItem::Playlist(id.to_owned()));
        }
    }

    /// Applies the edit and returns true if the library changed. The function ignores edits that
    /// are not possible now (an item or a folder that is gone, a folder into itself).
    pub fn apply(&mut self, edit: &LibraryEdit) -> bool {
        match edit {
            LibraryEdit::CreateFolder { id, name, parent } => {
                if self.folder(id).is_some() {
                    return false;
                }
                let folder = LibraryItem::Folder(Folder {
                    id: id.clone(),
                    name: name.clone(),
                    items: Vec::new(),
                });
                match parent {
                    None => self.items.insert(0, folder),
                    Some(parent) => match find_folder_mut(&mut self.items, parent) {
                        Some(parent) => parent.items.insert(0, folder),
                        None => return false,
                    },
                }
                true
            }
            LibraryEdit::RenameFolder { id, name } => match find_folder_mut(&mut self.items, id) {
                Some(folder) if &folder.name != name => {
                    folder.name.clone_from(name);
                    true
                }
                _ => false,
            },
            LibraryEdit::DeleteFolder { id } => {
                let key = LibraryKey::Folder(id.clone());
                let Some((items, index)) = locate_mut(&mut self.items, &key) else {
                    return false;
                };
                let LibraryItem::Folder(folder) = items.remove(index) else {
                    unreachable!("the key is a folder's");
                };
                items.splice(index..index, folder.items);
                true
            }
            LibraryEdit::Move { item, to, index } => self.move_item(item, to.as_deref(), *index),
        }
    }

    fn move_item(&mut self, key: &LibraryKey, to: Option<&str>, index: Option<usize>) -> bool {
        // A folder cannot go into itself or into one of its folders.
        if let (LibraryKey::Folder(moved), Some(to)) = (key, to)
            && self.path_to(to).iter().any(|f| &f.id == moved)
        {
            return false;
        }
        if to.is_some_and(|to| self.folder(to).is_none()) {
            return false;
        }
        let Some(from_parent) = self.parent_of(key).map(|p| p.map(str::to_owned)) else {
            return false;
        };
        let (items, from) = locate_mut(&mut self.items, key).expect("it has a parent");
        let item = items.remove(from);
        let target = match to {
            None => &mut self.items,
            Some(to) => {
                &mut find_folder_mut(&mut self.items, to)
                    .expect("checked above")
                    .items
            }
        };
        let at = index.unwrap_or(target.len()).min(target.len());
        target.insert(at, item);
        from_parent.as_deref() != to || from != at
    }
}

fn find_folder<'a>(items: &'a [LibraryItem], id: &str) -> Option<&'a Folder> {
    items.iter().find_map(|item| match item {
        LibraryItem::Folder(folder) if folder.id == id => Some(folder),
        LibraryItem::Folder(folder) => find_folder(&folder.items, id),
        LibraryItem::Playlist(_) => None,
    })
}

fn find_folder_mut<'a>(items: &'a mut [LibraryItem], id: &str) -> Option<&'a mut Folder> {
    for item in items {
        let LibraryItem::Folder(folder) = item else {
            continue;
        };
        if folder.id == id {
            return Some(folder);
        }
        if let Some(found) = find_folder_mut(&mut folder.items, id) {
            return Some(found);
        }
    }
    None
}

fn path_to<'a>(items: &'a [LibraryItem], id: &str, path: &mut Vec<&'a Folder>) -> bool {
    for item in items {
        if let LibraryItem::Folder(folder) = item {
            path.push(folder);
            if folder.id == id || path_to(&folder.items, id, path) {
                return true;
            }
            path.pop();
        }
    }
    false
}

fn parent_of<'a>(items: &'a [LibraryItem], key: &LibraryKey) -> Option<&'a str> {
    items.iter().find_map(|item| match item {
        LibraryItem::Folder(folder) if folder.items.iter().any(|child| child.is(key)) => {
            Some(folder.id.as_str())
        }
        LibraryItem::Folder(folder) => parent_of(&folder.items, key),
        LibraryItem::Playlist(_) => None,
    })
}

/// The list that holds `key`, and its index there.
fn locate_mut<'a>(
    items: &'a mut Vec<LibraryItem>,
    key: &LibraryKey,
) -> Option<(&'a mut Vec<LibraryItem>, usize)> {
    if let Some(index) = items.iter().position(|item| item.is(key)) {
        return Some((items, index));
    }
    items.iter_mut().find_map(|item| match item {
        LibraryItem::Folder(folder) => locate_mut(&mut folder.items, key),
        LibraryItem::Playlist(_) => None,
    })
}

fn collect_playlists(items: &[LibraryItem], ids: &mut Vec<String>) {
    for item in items {
        match item {
            LibraryItem::Playlist(id) => ids.push(id.clone()),
            LibraryItem::Folder(folder) => collect_playlists(&folder.items, ids),
        }
    }
}

fn collect_folders<'a>(items: &'a [LibraryItem], ids: &mut Vec<&'a str>) {
    for item in items {
        if let LibraryItem::Folder(folder) = item {
            ids.push(&folder.id);
            collect_folders(&folder.items, ids);
        }
    }
}

/// Removes the playlists that are not in `listed` and the repeated playlists (and repeated
/// `folders`). It records the remaining items in `seen`.
fn retain_in(
    items: &mut Vec<LibraryItem>,
    listed: &HashSet<String>,
    seen: &mut HashSet<String>,
    folders: &mut HashSet<String>,
) {
    items.retain_mut(|item| match item {
        LibraryItem::Playlist(id) => listed.contains(id) && seen.insert(id.clone()),
        LibraryItem::Folder(folder) => {
            if !folders.insert(folder.id.clone()) {
                return false;
            }
            retain_in(&mut folder.items, listed, seen, folders);
            true
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn p(id: &str) -> LibraryItem {
        LibraryItem::Playlist(id.to_owned())
    }

    fn f(id: &str, name: &str, items: Vec<LibraryItem>) -> LibraryItem {
        LibraryItem::Folder(Folder {
            id: id.to_owned(),
            name: name.to_owned(),
            items,
        })
    }

    /// Festivals [Resurrection Fest [rock], pop], queen, calm
    fn festivals() -> Library {
        Library::new(vec![
            f(
                "folder-1",
                "Festivals",
                vec![
                    f("folder-2", "Resurrection Fest", vec![p("rock")]),
                    p("pop"),
                ],
            ),
            p("queen"),
            p("calm"),
        ])
    }

    #[test]
    fn a_library_without_folders_keeps_spotifys_order() {
        let mut library = Library::default();
        let arranged = library.arrange(playlists(&["a", "b", "c"]));
        assert_eq!(ids(&arranged), ["a", "b", "c"]);
        assert_eq!(library, Library::new(vec![p("a"), p("b"), p("c")]));
    }

    #[test]
    fn playlists_are_listed_going_into_each_folder() {
        let mut library = festivals();
        let arranged = library.arrange(playlists(&["calm", "pop", "queen", "rock"]));
        assert_eq!(ids(&arranged), ["rock", "pop", "queen", "calm"]);
        assert_eq!(library, festivals(), "nothing to fit");
    }

    #[test]
    fn new_playlists_follow_their_neighbor_and_gone_ones_leave() {
        let mut library = festivals();
        // "metal" follows "rock" in Spotify's order. "new" comes first.
        let arranged = library.arrange(playlists(&["new", "rock", "metal", "pop", "calm"]));
        assert_eq!(ids(&arranged), ["new", "rock", "metal", "pop", "calm"]);
        assert_eq!(
            library,
            Library::new(vec![
                p("new"),
                f(
                    "folder-1",
                    "Festivals",
                    vec![
                        f("folder-2", "Resurrection Fest", vec![p("rock"), p("metal")]),
                        p("pop"),
                    ],
                ),
                p("calm"),
            ])
        );
    }

    #[test]
    fn repeated_entries_are_left_out() {
        let mut library = Library::new(vec![
            p("a"),
            f("folder-1", "One", vec![p("a"), p("b")]),
            f("folder-1", "Copy", vec![]),
        ]);
        let arranged = library.arrange(playlists(&["a", "b"]));
        assert_eq!(ids(&arranged), ["a", "b"]);
        assert_eq!(
            library,
            Library::new(vec![p("a"), f("folder-1", "One", vec![p("b")])])
        );
    }

    #[test]
    fn folders_are_found_with_their_path_and_contents() {
        let library = festivals();
        let path: Vec<&str> = library
            .path_to("folder-2")
            .iter()
            .map(|f| f.name.as_str())
            .collect();
        assert_eq!(path, ["Festivals", "Resurrection Fest"]);
        assert!(library.path_to("nope").is_empty());
        assert_eq!(library.level(Some("folder-2")), Some(&[p("rock")][..]));
        assert_eq!(library.level(Some("nope")), None);
        assert_eq!(
            library.parent_of(&LibraryKey::Playlist("rock".into())),
            Some(Some("folder-2"))
        );
        assert_eq!(
            library.parent_of(&LibraryKey::Folder("folder-1".into())),
            Some(None)
        );
        let festivals = library.folder("folder-1").unwrap();
        assert_eq!(festivals.first_playlist(), Some("rock"));
        assert_eq!(festivals.count_line(), "Folder · 2 playlists");
        assert!(festivals.holds_playlist("rock"));
        assert!(!festivals.holds_playlist("calm"));
        assert_eq!(library.next_folder_id(), "folder-3");
    }

    #[test]
    fn folders_are_created_renamed_and_deleted() {
        let mut library = festivals();
        let create = LibraryEdit::CreateFolder {
            id: "folder-3".into(),
            name: "Viajes".into(),
            parent: Some("folder-1".into()),
        };
        assert!(library.apply(&create));
        assert!(!library.apply(&create), "already there");
        assert_eq!(
            library.level(Some("folder-1")).unwrap()[0],
            f("folder-3", "Viajes", vec![])
        );
        assert_eq!(
            library.folder("folder-3").unwrap().count_line(),
            "Folder · Empty"
        );

        assert!(library.apply(&LibraryEdit::RenameFolder {
            id: "folder-3".into(),
            name: "Road trips".into(),
        }));
        assert_eq!(library.folder("folder-3").unwrap().name, "Road trips");

        // Its contents go to its position.
        assert!(library.apply(&LibraryEdit::DeleteFolder {
            id: "folder-1".into()
        }));
        assert_eq!(
            library,
            Library::new(vec![
                f("folder-3", "Road trips", vec![]),
                f("folder-2", "Resurrection Fest", vec![p("rock")]),
                p("pop"),
                p("queen"),
                p("calm"),
            ])
        );
        assert!(!library.apply(&LibraryEdit::DeleteFolder {
            id: "folder-1".into()
        }));
    }

    #[test]
    fn items_move_into_folders_and_within_a_level() {
        let mut library = festivals();
        // Into a folder: at its end.
        assert!(library.apply(&LibraryEdit::Move {
            item: LibraryKey::Playlist("queen".into()),
            to: Some("folder-2".into()),
            index: None,
        }));
        assert_eq!(
            library.level(Some("folder-2")),
            Some(&[p("rock"), p("queen")][..])
        );
        // Out to the first level, at a position.
        assert!(library.apply(&LibraryEdit::Move {
            item: LibraryKey::Folder("folder-2".into()),
            to: None,
            index: Some(1),
        }));
        assert_eq!(
            library,
            Library::new(vec![
                f("folder-1", "Festivals", vec![p("pop")]),
                f("folder-2", "Resurrection Fest", vec![p("rock"), p("queen")]),
                p("calm"),
            ])
        );
        // In its level.
        assert!(library.apply(&LibraryEdit::Move {
            item: LibraryKey::Playlist("calm".into()),
            to: None,
            index: Some(0),
        }));
        assert_eq!(library.playlist_ids(), ["calm", "pop", "rock", "queen"]);
        // At its current position: nothing changes.
        assert!(!library.apply(&LibraryEdit::Move {
            item: LibraryKey::Playlist("calm".into()),
            to: None,
            index: Some(0),
        }));
    }

    #[test]
    fn a_folder_cannot_go_into_itself() {
        let mut library = festivals();
        for to in ["folder-1", "folder-2", "nope"] {
            assert!(!library.apply(&LibraryEdit::Move {
                item: LibraryKey::Folder("folder-1".into()),
                to: Some(to.into()),
                index: None,
            }));
        }
        assert!(!library.apply(&LibraryEdit::Move {
            item: LibraryKey::Playlist("gone".into()),
            to: None,
            index: None,
        }));
        assert_eq!(library, festivals());
    }

    #[test]
    fn a_new_playlist_can_start_in_a_folder() {
        let mut library = festivals();
        library.place_new("new", "folder-2");
        library.place_new("new", "folder-1");
        let arranged = library.arrange(playlists(&["new", "rock", "pop", "queen", "calm"]));
        assert_eq!(ids(&arranged), ["new", "rock", "pop", "queen", "calm"]);
        assert_eq!(
            library.level(Some("folder-2")),
            Some(&[p("new"), p("rock")][..])
        );
    }

    #[test]
    fn the_library_is_saved_as_plain_json() {
        let json = serde_json::to_value(festivals().items()).unwrap();
        assert_eq!(
            json,
            serde_json::json!([
                {"folder": {"id": "folder-1", "name": "Festivals", "items": [
                    {"folder": {"id": "folder-2", "name": "Resurrection Fest", "items": [
                        {"playlist": "rock"}
                    ]}},
                    {"playlist": "pop"}
                ]}},
                {"playlist": "queen"},
                {"playlist": "calm"}
            ])
        );
        let read: Vec<LibraryItem> = serde_json::from_value(json).unwrap();
        assert_eq!(Library::new(read), festivals());
    }
}
