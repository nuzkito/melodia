//! The demo backend implements the full protocol with fake data.

use std::time::Duration;

use melodia::backend::{
    self, AuthStatus, BackendHandle, Command, Event, QueueAt, Timings, WebPlayerStatus, demo,
};
use melodia::model::{
    LibraryEdit, LibraryKey, PlaylistEdit, PlaylistsStatus, QueueEdit, QueuePosition, ReleaseKind,
};
use melodia::webplayer::DEVICE_NAME;

/// The simulated delays of the demo, made shorter so that the tests do not wait for them.
fn timings() -> Timings {
    Timings::scaled(0.1)
}

async fn wait_for(handle: &BackendHandle, mut predicate: impl FnMut(&Event) -> bool) -> Event {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        while let Some(event) = handle.try_recv() {
            if predicate(&event) {
                return event;
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for an event"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn demo_supports_the_whole_flow() {
    let (handle, sink, commands) = backend::channel(|| {});
    let task = tokio::spawn(demo::run(commands, sink, timings()));

    wait_for(&handle, |e| {
        matches!(e, Event::Auth(AuthStatus::LoggedOut { .. }))
    })
    .await;
    handle.send(Command::Login { client_id: None });
    wait_for(&handle, |e| {
        matches!(e, Event::Auth(AuthStatus::LoggedIn { demo: true }))
    })
    .await;
    let Event::Queue(queue) = wait_for(&handle, |e| matches!(e, Event::Queue(_))).await else {
        unreachable!()
    };
    let first = queue.current.clone().unwrap();
    assert!(
        queue.previous.is_empty() && !queue.next.is_empty(),
        "recently played tracks are not shown"
    );

    // Next: the current track becomes the last of the history.
    handle.send(Command::Next);
    let Event::Queue(queue) = wait_for(
        &handle,
        |e| matches!(e, Event::Queue(q) if q.current.as_ref() != Some(&first)),
    )
    .await
    else {
        unreachable!()
    };
    assert_eq!(queue.previous.last(), Some(&first));

    // Previous goes back to it.
    handle.send(Command::Seek { position_ms: 0 });
    handle.send(Command::Previous);
    wait_for(
        &handle,
        |e| matches!(e, Event::Queue(q) if q.current.as_ref() == Some(&first)),
    )
    .await;

    // Search without case sensitivity, with pages.
    handle.send(Command::Search {
        query: "ORBIT".into(),
        offset: 0,
    });
    let Event::SearchResults(page) =
        wait_for(&handle, |e| matches!(e, Event::SearchResults(_))).await
    else {
        unreachable!()
    };
    assert_eq!(page.tracks.len(), 4);
    assert!(page.tracks.iter().all(|t| t.album == "Low Orbit"));
    assert!(!page.has_more());

    // Albums and artists come only with the first page.
    assert_eq!(page.albums.len(), 1);
    assert_eq!(page.albums[0].release.name, "Low Orbit");
    assert_eq!(
        page.albums[0].subtitle(),
        format!("Album · {}", page.tracks[0].artists[0])
    );
    assert!(page.artists.is_empty());
    handle.send(Command::Search {
        query: page.tracks[0].artists[0].clone(),
        offset: 0,
    });
    let Event::SearchResults(by_artist) =
        wait_for(&handle, |e| matches!(e, Event::SearchResults(_))).await
    else {
        unreachable!()
    };
    assert_eq!(by_artist.artists.len(), 1);
    assert_eq!(by_artist.artists[0].name, page.tracks[0].artists[0]);

    // Two tracks played here first: the history starts empty, so they make the history.
    handle.send(Command::Next);
    handle.send(Command::Next);
    wait_for(
        &handle,
        |e| matches!(e, Event::Queue(q) if q.previous.len() == 2),
    )
    .await;

    // ＋ adds at the end of the queue. A drag puts a track at any position.
    let queued = page.tracks[2].clone();
    handle.send(Command::AddToQueue {
        track: queued.clone(),
    });
    let Event::Queue(queue) = wait_for(
        &handle,
        |e| matches!(e, Event::Queue(q) if q.next.last() == Some(&queued)),
    )
    .await
    else {
        unreachable!()
    };
    // Positions count the full queue: the previous tracks, then the current track.
    let first_next = queue.previous.len() + 1;
    handle.send(Command::EditQueue(QueueEdit::Move {
        from: first_next + queue.next.len() - 1,
        to: first_next,
        uri: queued.uri.clone(),
    }));
    wait_for(
        &handle,
        |e| matches!(e, Event::Queue(q) if q.edits == 1 && q.next.first() == Some(&queued)),
    )
    .await;
    let inserted = page.tracks[3].clone();
    handle.send(Command::EditQueue(QueueEdit::Insert {
        index: 1,
        track: inserted.clone(),
    }));
    let Event::Queue(queue) = wait_for(
        &handle,
        |e| matches!(e, Event::Queue(q) if q.edits == 2 && q.next.get(1) == Some(&inserted)),
    )
    .await
    else {
        unreachable!()
    };

    // ✕ on a previous track removes it from the history.
    let removed = queue.previous[0].clone();
    let previous_count = queue.previous.len();
    handle.send(Command::EditQueue(QueueEdit::RemovePrevious {
        index: 0,
        uri: removed.uri.clone(),
    }));
    let Event::Queue(queue) =
        wait_for(&handle, |e| matches!(e, Event::Queue(q) if q.edits == 3)).await
    else {
        unreachable!()
    };
    assert_eq!(queue.previous.len(), previous_count - 1);

    // When a queued track plays, the queue jumps to it. The tracks before it become history.
    handle.send(Command::PlayFromQueue {
        position: QueuePosition::Next(1),
        uri: inserted.uri.clone(),
    });
    let Event::Queue(queue) = wait_for(
        &handle,
        |e| matches!(e, Event::Queue(q) if q.current.as_ref() == Some(&inserted)),
    )
    .await
    else {
        unreachable!()
    };
    assert_eq!(queue.previous.last(), Some(&queued));

    // Volume.
    handle.send(Command::SetVolume { percent: 35 });
    wait_for(&handle, |e| {
        matches!(e, Event::Playback(p) if p.device.as_ref().and_then(|d| d.volume_percent) == Some(35))
    })
    .await;

    // The demo makes the covers locally.
    let url = queued.cover_large.clone().unwrap();
    handle.send(Command::LoadImage {
        url: url.clone(),
        max_px: 640,
    });
    let Event::ImageLoaded { image, .. } = wait_for(
        &handle,
        |e| matches!(e, Event::ImageLoaded { url: u, .. } if *u == url),
    )
    .await
    else {
        unreachable!()
    };
    assert_eq!((image.width, image.height), (640, 640));
    assert!(image.accent.is_some());

    // «Play in this app» is simulated (the demo has no audio).
    handle.send(Command::StartWebPlayer);
    wait_for(&handle, |e| {
        matches!(e, Event::WebPlayer(WebPlayerStatus::Ready { .. }))
    })
    .await;
    wait_for(&handle, |e| {
        matches!(e, Event::Playback(p) if p.device.as_ref().is_some_and(|d| d.name == DEVICE_NAME))
    })
    .await;

    // Clear empties the full queue and stops the music.
    handle.send(Command::ClearQueue);
    wait_for(
        &handle,
        |e| matches!(e, Event::Playback(p) if p.track.is_none() && !p.is_playing),
    )
    .await;
    wait_for(&handle, |e| {
        matches!(e, Event::Queue(q) if q.edits == 4
            && q.previous.is_empty() && q.current.is_none() && q.next.is_empty())
    })
    .await;

    handle.send(Command::Logout);
    wait_for(&handle, |e| {
        matches!(e, Event::Auth(AuthStatus::LoggedOut { .. }))
    })
    .await;
    drop(handle);
    task.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn demo_playlists_can_be_queued_and_filled() {
    let (handle, sink, commands) = backend::channel(|| {});
    let task = tokio::spawn(demo::run(commands, sink, timings()));
    handle.send(Command::Login { client_id: None });
    // The queue comes first, then the playlists.
    let Event::Queue(before) = wait_for(&handle, |e| matches!(e, Event::Queue(_))).await else {
        unreachable!()
    };
    let Event::Playlists(PlaylistsStatus::Loaded(playlists)) = wait_for(&handle, |e| {
        matches!(e, Event::Playlists(PlaylistsStatus::Loaded(_)))
    })
    .await
    else {
        unreachable!()
    };
    let calm = playlists.iter().find(|p| p.name == "Calm").unwrap().clone();
    assert!(calm.editable && calm.total == 5);
    let theirs = playlists.iter().find(|p| !p.editable).unwrap().clone();

    // The full playlist goes in the queue, after the first upcoming track.
    handle.send(Command::QueuePlaylist {
        playlist: calm.clone(),
        index: 1,
    });
    let Event::Queue(queue) = wait_for(
        &handle,
        |e| matches!(e, Event::Queue(q) if q.next.len() == before.next.len() + 5),
    )
    .await
    else {
        unreachable!()
    };
    assert_eq!(queue.next[0], before.next[0]);
    assert_eq!(queue.next[1].name, "Fog");
    assert_eq!(queue.next[6], before.next[1]);

    // A track added to it, which it did not have.
    let track = queue.next[0].clone();
    handle.send(Command::FindInPlaylist {
        playlist: calm.clone(),
        index: None,
        track: track.clone(),
    });
    wait_for(
        &handle,
        |e| matches!(e, Event::FoundInPlaylist { track: t, found: false, .. } if *t == track),
    )
    .await;
    handle.send(Command::EditPlaylist {
        playlist: calm.clone(),
        edit: PlaylistEdit::Insert { index: None, track },
    });
    wait_for(&handle, |e| {
        matches!(e, Event::Playlists(PlaylistsStatus::Loaded(lists))
            if lists.iter().any(|p| p.id == calm.id && p.total == 6))
    })
    .await;
    // Now it has it.
    handle.send(Command::FindInPlaylist {
        playlist: calm.clone(),
        index: None,
        track: queue.next[0].clone(),
    });
    wait_for(&handle, |e| {
        matches!(e, Event::FoundInPlaylist { found: true, .. })
    })
    .await;

    // The playlist of a different user cannot be used.
    handle.send(Command::QueuePlaylist {
        playlist: theirs.clone(),
        index: 0,
    });
    wait_for(
        &handle,
        |e| matches!(e, Event::Toast(t) if t.message.contains("only your own playlists")),
    )
    .await;

    // The UI can show its tracks, but not the tracks of the playlist of a different user.
    handle.send(Command::LoadPlaylistTracks {
        playlist: calm.clone(),
    });
    let Event::PlaylistTracks {
        id,
        tracks,
        total,
        edits,
    } = wait_for(&handle, |e| matches!(e, Event::PlaylistTracks { .. })).await
    else {
        unreachable!()
    };
    assert_eq!(id, calm.id);
    assert_eq!(tracks.len(), 6);
    assert_eq!(total, 6);
    assert_eq!(tracks[0].name, "Fog");
    assert_eq!(edits, 1, "the edits received so far");

    // The order of its tracks changes, and tracks are removed one copy at a time.
    let fog = tracks[0].clone();
    let edits = [
        // A second "Fog", in third position.
        PlaylistEdit::Insert {
            index: Some(2),
            track: fog.clone(),
        },
        // The last track goes first.
        PlaylistEdit::Move {
            from: 6,
            to: 0,
            uri: tracks[5].uri.clone(),
        },
        // The first "Fog" (now second) goes.
        PlaylistEdit::Remove {
            index: 1,
            uri: fog.uri.clone(),
        },
    ];
    for edit in edits {
        handle.send(Command::EditPlaylist {
            playlist: calm.clone(),
            edit,
        });
    }
    handle.send(Command::LoadPlaylistTracks {
        playlist: calm.clone(),
    });
    let Event::PlaylistTracks {
        tracks: edited,
        edits,
        ..
    } = wait_for(
        &handle,
        |e| matches!(e, Event::PlaylistTracks { edits, .. } if *edits == 4),
    )
    .await
    else {
        unreachable!()
    };
    assert_eq!(edits, 4);
    let names: Vec<&str> = edited.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names,
        [
            tracks[5].name.as_str(),
            "Pine Heart",
            "Fog",
            "Green Tea",
            "April Rain",
            "Aurora"
        ]
    );
    handle.send(Command::LoadPlaylistTracks {
        playlist: theirs.clone(),
    });
    wait_for(
        &handle,
        |e| matches!(e, Event::PlaylistTracksFailed { id, .. } if *id == theirs.id),
    )
    .await;

    drop(handle);
    task.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn demo_playlists_keep_the_order_given_to_them() {
    let (handle, sink, commands) = backend::channel(|| {});
    let task = tokio::spawn(demo::run(commands, sink, timings()));
    handle.send(Command::Login { client_id: None });
    let Event::Playlists(PlaylistsStatus::Loaded(playlists)) = wait_for(&handle, |e| {
        matches!(e, Event::Playlists(PlaylistsStatus::Loaded(_)))
    })
    .await
    else {
        unreachable!()
    };
    // The last playlist goes to the top.
    let last = playlists.last().unwrap().id.clone();
    handle.send(Command::EditLibrary(LibraryEdit::Move {
        item: LibraryKey::Playlist(last.clone()),
        to: None,
        index: Some(0),
    }));
    let mut ids: Vec<String> = playlists.iter().map(|p| p.id.clone()).collect();
    ids.rotate_right(1);

    // The demo loads it again after an edit, and on request: in the new order.
    let calm = playlists.iter().find(|p| p.name == "Calm").unwrap().clone();
    let track = demo::catalog()[0].clone();
    handle.send(Command::EditPlaylist {
        playlist: calm.clone(),
        edit: PlaylistEdit::Insert { index: None, track },
    });
    let Event::Playlists(PlaylistsStatus::Loaded(edited)) = wait_for(&handle, |e| {
        matches!(e, Event::Playlists(PlaylistsStatus::Loaded(lists))
            if lists.iter().any(|p| p.id == calm.id && p.total == 6))
    })
    .await
    else {
        unreachable!()
    };
    assert_eq!(
        edited.iter().map(|p| &p.id).collect::<Vec<_>>(),
        ids.iter().collect::<Vec<_>>()
    );
    handle.send(Command::RefreshPlaylists);
    let Event::Playlists(PlaylistsStatus::Loaded(refreshed)) = wait_for(&handle, |e| {
        matches!(e, Event::Playlists(PlaylistsStatus::Loaded(_)))
    })
    .await
    else {
        unreachable!()
    };
    assert_eq!(refreshed, edited);

    drop(handle);
    task.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn demo_renames_the_users_playlists() {
    let (handle, sink, commands) = backend::channel(|| {});
    let task = tokio::spawn(demo::run(commands, sink, timings()));
    handle.send(Command::Login { client_id: None });
    let Event::Playlists(PlaylistsStatus::Loaded(playlists)) = wait_for(&handle, |e| {
        matches!(e, Event::Playlists(PlaylistsStatus::Loaded(_)))
    })
    .await
    else {
        unreachable!()
    };
    let mine = playlists.iter().find(|p| p.editable).unwrap().clone();

    handle.send(Command::RenamePlaylist {
        playlist: mine.clone(),
        name: "Road Trip".into(),
    });
    let Event::Playlists(PlaylistsStatus::Loaded(renamed)) = wait_for(&handle, |e| {
        matches!(e, Event::Playlists(PlaylistsStatus::Loaded(_)))
    })
    .await
    else {
        unreachable!()
    };
    let listed = renamed.iter().find(|p| p.id == mine.id).unwrap();
    assert_eq!(listed.name, "Road Trip");
    assert_eq!(renamed.len(), playlists.len());

    drop(handle);
    task.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn demo_creates_empty_playlists_at_the_top() {
    let (handle, sink, commands) = backend::channel(|| {});
    let task = tokio::spawn(demo::run(commands, sink, timings()));
    handle.send(Command::Login { client_id: None });
    wait_for(&handle, |e| {
        matches!(e, Event::Playlists(PlaylistsStatus::Loaded(_)))
    })
    .await;

    handle.send(Command::CreatePlaylist {
        name: "Road Trip".into(),
        folder: None,
    });
    // The UI gets a message about the creation, and then the list has it.
    let Event::PlaylistCreated { name, id } =
        wait_for(&handle, |e| matches!(e, Event::PlaylistCreated { .. })).await
    else {
        unreachable!()
    };
    assert_eq!(name, "Road Trip");
    let Event::Playlists(PlaylistsStatus::Loaded(playlists)) = wait_for(&handle, |e| {
        matches!(e, Event::Playlists(PlaylistsStatus::Loaded(lists))
            if lists.iter().any(|p| p.name == "Road Trip"))
    })
    .await
    else {
        unreachable!()
    };
    let new = &playlists[0];
    assert_eq!(id.as_ref(), Some(&new.id));
    assert_eq!(new.name, "Road Trip");
    assert_eq!(new.total, 0);
    assert!(new.editable);
    assert_eq!(
        playlists.iter().filter(|p| p.id == new.id).count(),
        1,
        "a new ID"
    );

    drop(handle);
    task.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn demo_playlists_can_be_created_in_a_folder() {
    let (handle, sink, commands) = backend::channel(|| {});
    let task = tokio::spawn(demo::run(commands, sink, timings()));
    handle.send(Command::Login { client_id: None });
    let Event::Library { library, .. } =
        wait_for(&handle, |e| matches!(e, Event::Library { .. })).await
    else {
        unreachable!()
    };
    wait_for(&handle, |e| {
        matches!(e, Event::Playlists(PlaylistsStatus::Loaded(_)))
    })
    .await;

    // As the sidebar does: the demo library already has a folder.
    let id = library.next_folder_id();
    handle.send(Command::EditLibrary(LibraryEdit::CreateFolder {
        id: id.clone(),
        name: "Concerts".into(),
        parent: None,
    }));
    handle.send(Command::CreatePlaylist {
        name: "Resurrection Fest".into(),
        folder: Some(id.clone()),
    });
    let Event::Library { library, edits } = wait_for(&handle, |e| {
        matches!(e, Event::Library { library, .. }
            if library.folder(&id).is_some_and(|f| f.playlist_count() == 1))
    })
    .await
    else {
        unreachable!()
    };
    assert_eq!(edits, 1);
    let folder = library.folder(&id).expect("the folder is kept");
    assert_eq!(folder.name, "Concerts");
    let Event::Playlists(PlaylistsStatus::Loaded(playlists)) = wait_for(&handle, |e| {
        matches!(e, Event::Playlists(PlaylistsStatus::Loaded(_)))
    })
    .await
    else {
        unreachable!()
    };
    // Listed first: its folder is at the top.
    assert_eq!(playlists[0].name, "Resurrection Fest");
    assert_eq!(folder.first_playlist(), Some(playlists[0].id.as_str()));

    drop(handle);
    task.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_album_of_the_current_track_can_be_opened() {
    let (handle, sink, commands) = backend::channel(|| {});
    let task = tokio::spawn(demo::run(commands, sink, timings()));
    handle.send(Command::Login { client_id: None });
    let Event::Queue(queue) = wait_for(
        &handle,
        |e| matches!(e, Event::Queue(q) if q.current.is_some()),
    )
    .await
    else {
        unreachable!()
    };
    let current = queue.current.unwrap();
    let id = current.album_id.clone().expect("demo tracks have an album");

    handle.send(Command::LoadAlbum { id: id.clone() });
    let Event::Album(album) = wait_for(&handle, |e| matches!(e, Event::Album(_))).await else {
        unreachable!()
    };
    assert_eq!(album.id, id);
    assert_eq!(album.name, current.album);
    assert!(album.year.is_some());
    assert!(album.tracks.contains(&current));
    assert!(album.tracks.iter().all(|t| t.album == current.album));

    handle.send(Command::LoadAlbum {
        id: "missing".into(),
    });
    wait_for(
        &handle,
        |e| matches!(e, Event::AlbumFailed { id, .. } if id == "missing"),
    )
    .await;
    task.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_artist_opens_with_their_releases_which_can_be_queued_or_added_to_playlists() {
    let (handle, sink, commands) = backend::channel(|| {});
    let task = tokio::spawn(demo::run(commands, sink, timings()));
    handle.send(Command::Login { client_id: None });
    let Event::Queue(queue) = wait_for(
        &handle,
        |e| matches!(e, Event::Queue(q) if q.current.is_some()),
    )
    .await
    else {
        unreachable!()
    };
    let current = queue.current.unwrap();
    let id = current
        .artist_id(0)
        .expect("demo tracks have an artist")
        .to_owned();

    handle.send(Command::LoadArtist { id: id.clone() });
    let Event::Artist(artist) = wait_for(&handle, |e| matches!(e, Event::Artist(_))).await else {
        unreachable!()
    };
    assert_eq!(artist.id, id);
    assert_eq!(artist.name, current.artists[0]);
    let kinds: Vec<_> = artist.releases.iter().map(|r| r.kind).collect();
    assert!(kinds.contains(&ReleaseKind::Album) && kinds.contains(&ReleaseKind::Single));
    assert!(
        artist
            .releases
            .iter()
            .any(|r| Some(&r.id) == current.album_id.as_ref())
    );
    // The newest first, albums and singles together.
    let dates: Vec<_> = artist
        .releases
        .iter()
        .map(|r| r.release_date.as_str())
        .collect();
    assert!(dates.is_sorted_by(|a, b| a >= b));

    // A release dropped on the queue puts all its songs there.
    let single = artist
        .releases
        .iter()
        .find(|r| r.kind == ReleaseKind::Single)
        .unwrap()
        .clone();
    let count = single.total_tracks as usize;
    handle.send(Command::QueueAlbum {
        id: single.id.clone(),
        name: single.name.clone(),
        at: QueueAt::Next(0),
    });
    wait_for(&handle, |e| {
        matches!(e, Event::Queue(q) if q.next.len() >= count
            && q.next[..count].iter().all(|t| t.album_id.as_ref() == Some(&single.id)))
    })
    .await;

    // «Play» in its menu: its first song plays now, and the other songs immediately after.
    handle.send(Command::QueueAlbum {
        id: single.id.clone(),
        name: single.name.clone(),
        at: QueueAt::Now,
    });
    wait_for(&handle, |e| {
        matches!(e, Event::Queue(q) if q.current.as_ref().is_some_and(|t| t.album_id.as_ref() == Some(&single.id))
            && q.next[..count - 1].iter().all(|t| t.album_id.as_ref() == Some(&single.id)))
    })
    .await;

    // Dropped on a playlist: its songs, and the songs that the playlist has.
    handle.send(Command::RefreshPlaylists);
    let calm = match wait_for(&handle, |e| {
        matches!(e, Event::Playlists(PlaylistsStatus::Loaded(_)))
    })
    .await
    {
        Event::Playlists(PlaylistsStatus::Loaded(lists)) => {
            lists.into_iter().find(|p| p.editable).unwrap()
        }
        _ => unreachable!(),
    };
    handle.send(Command::FindAlbumInPlaylist {
        playlist: calm.clone(),
        id: single.id.clone(),
        name: single.name.clone(),
    });
    let Event::FoundAlbumInPlaylist { tracks, .. } =
        wait_for(&handle, |e| matches!(e, Event::FoundAlbumInPlaylist { .. })).await
    else {
        unreachable!()
    };
    assert_eq!(tracks.len(), count);
    handle.send(Command::EditPlaylist {
        playlist: calm.clone(),
        edit: PlaylistEdit::Append { tracks },
    });
    handle.send(Command::FindAlbumInPlaylist {
        playlist: calm.clone(),
        id: single.id.clone(),
        name: single.name.clone(),
    });
    wait_for(
        &handle,
        |e| matches!(e, Event::FoundAlbumInPlaylist { found, .. } if found.len() == count),
    )
    .await;

    handle.send(Command::LoadArtist {
        id: "missing".into(),
    });
    wait_for(
        &handle,
        |e| matches!(e, Event::ArtistFailed { id, .. } if id == "missing"),
    )
    .await;
    task.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn demo_keeps_liked_songs() {
    let (handle, sink, commands) = backend::channel(|| {});
    let task = tokio::spawn(demo::run(commands, sink, timings()));
    handle.send(Command::Login { client_id: None });
    wait_for(&handle, |e| {
        matches!(e, Event::Auth(AuthStatus::LoggedIn { demo: true }))
    })
    .await;
    let tracks = demo::catalog();
    let (liked, other) = (&tracks[0], &tracks[1]);
    let check = || Command::CheckLiked {
        uris: vec![liked.uri.clone(), other.uri.clone()],
    };

    handle.send(check());
    let Event::LikedStates(states) =
        wait_for(&handle, |e| matches!(e, Event::LikedStates(_))).await
    else {
        unreachable!()
    };
    assert_eq!(
        states,
        vec![(liked.uri.clone(), true), (other.uri.clone(), false)]
    );

    handle.send(Command::SetLiked {
        track: liked.clone(),
        liked: false,
    });
    handle.send(Command::SetLiked {
        track: other.clone(),
        liked: true,
    });
    handle.send(check());
    let Event::LikedStates(states) =
        wait_for(&handle, |e| matches!(e, Event::LikedStates(_))).await
    else {
        unreachable!()
    };
    assert_eq!(
        states,
        vec![(liked.uri.clone(), false), (other.uri.clone(), true)]
    );

    drop(handle);
    task.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn music_sent_to_the_phone_is_not_controlled_until_it_comes_back() {
    let (handle, sink, commands) = backend::channel(|| {});
    let task = tokio::spawn(demo::run(commands, sink, timings()));
    handle.send(Command::Login { client_id: None });
    let Event::Queue(queue) = wait_for(
        &handle,
        |e| matches!(e, Event::Queue(q) if q.current.is_some()),
    )
    .await
    else {
        unreachable!()
    };
    let playing = queue.current.unwrap();
    handle.send(Command::RefreshDevices);
    let Event::Devices(devices) = wait_for(&handle, |e| matches!(e, Event::Devices(_))).await
    else {
        unreachable!()
    };
    let phone = devices
        .into_iter()
        .find(|d| d.name == "Nuria's phone")
        .expect("the demo has a phone");

    handle.send(Command::TransferPlayback {
        device_id: phone.id.clone(),
    });
    let Event::PlayingElsewhere(Some(device)) =
        wait_for(&handle, |e| matches!(e, Event::PlayingElsewhere(Some(_)))).await
    else {
        unreachable!()
    };
    assert_eq!(device.id, phone.id);
    wait_for(&handle, |e| {
        matches!(e, Event::Queue(q) if q.previous.is_empty() && q.current.is_none() && q.next.is_empty())
    })
    .await;

    // The app controls nothing there.
    handle.send(Command::Next);
    handle.send(Command::Resume);
    // The demo handles commands in order. When the devices come back, it also handled the two
    // commands above.
    handle.send(Command::RefreshDevices);
    wait_for(&handle, |event| {
        if let Event::Playback(p) = event {
            assert!(p.track.is_none() && !p.is_playing, "{p:?}");
        }
        assert!(
            !matches!(event, Event::Queue(q) if q.current.is_some()),
            "{event:?}"
        );
        matches!(event, Event::Devices(_))
    })
    .await;

    // The music comes back: what the phone played, with its queue.
    handle.send(Command::StartWebPlayer);
    wait_for(&handle, |e| matches!(e, Event::PlayingElsewhere(None))).await;
    let Event::Queue(queue) = wait_for(
        &handle,
        |e| matches!(e, Event::Queue(q) if q.current.is_some()),
    )
    .await
    else {
        unreachable!()
    };
    assert_eq!(queue.current, Some(playing));
    assert!(!queue.next.is_empty() && queue.previous.is_empty());

    drop(handle);
    task.await.unwrap();
}
