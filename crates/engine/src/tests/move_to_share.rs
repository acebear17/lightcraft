//! Moving photos to a folder on a NAS or network share (`photo.moveToFolder`, which dropping them
//! on a Folders row runs), where the share can go away between the move and its undo:
//!
//! * The share offline when Undo runs: Undo is refused with the reason and keeps the step;
//!   nothing is deleted, the library still points at the files where they are, and once the share
//!   is back the same Undo moves them home.
//! * The share read-only when Undo runs: the same.
//! * The connection lost partway through a move: the files already moved go back, the library is
//!   unchanged and no history step is left.
//! * One file the share can't give back during Undo: the files already brought back go out again,
//!   so every photo points at its file and the step can run once the share answers.
//! * Across devices (a share is another volume): the file is copied, checked and only then
//!   removed; the copy goes to a hidden name and takes the photo's name only once complete.
//! * A folder moved across devices (`folder.move`): the same, for everything in it; anything
//!   that isn't a plain file or folder refuses the move before anything is copied.

use lightcraft_catalog::{Op, Photo, PhotoId, Source};
use serde_json::json;

use crate::Session;

/// A scratch folder that goes away with the test, however it ends.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new(tag: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("lc-share-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("local")).unwrap();
        std::fs::create_dir_all(dir.join("share")).unwrap();
        Scratch(dir)
    }
    fn path(&self, rel: &str) -> String {
        self.0.join(rel).components().collect::<std::path::PathBuf>().to_string_lossy().to_string()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn add_file(s: &mut Session, path: &str, body: &str) -> PhotoId {
    std::fs::write(path, body).unwrap();
    let id = s.catalog.alloc_photo_id();
    let name = std::path::Path::new(path).file_name().unwrap().to_string_lossy().to_string();
    let p = Photo::new(id, Source::File { path: path.into() }, &name, "JPEG", 60, 40, "2026-01-01T10:00:00");
    s.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    id
}

fn file_of(s: &Session, id: PhotoId) -> String {
    match &s.catalog.photo(id).unwrap().source {
        Source::File { path } => path.clone(),
        _ => panic!("not a file"),
    }
}

/// Two photos on the local disk moved to the share.
fn moved_to_share(dir: &Scratch) -> (Session, PhotoId, PhotoId) {
    let mut s = Session::new();
    let a = add_file(&mut s, &dir.path("local/a.jpg"), "A");
    std::fs::write(dir.path("local/a.xmp"), "<edits/>").unwrap();
    let b = add_file(&mut s, &dir.path("local/b.jpg"), "B");
    let r = s.execute("photo.moveToFolder", &json!({"ids": [a.0, b.0], "folder": dir.path("share")})).unwrap();
    assert_eq!(r["moved"], 2);
    (s, a, b)
}

#[test]
fn undo_with_the_share_offline_is_refused_keeps_the_step_and_works_once_it_is_back() {
    let dir = Scratch::new("offline");
    let (mut s, a, b) = moved_to_share(&dir);
    let steps = s.undo.len();
    // the share goes offline: its folder is no longer there
    let away = dir.0.join("share-unmounted");
    std::fs::rename(dir.0.join("share"), &away).unwrap();
    let e = s.execute("edit.undo", &json!({})).unwrap_err().to_string();
    assert!(e.contains("can't move the files back"), "says why: {e}");
    assert_eq!(s.undo.len(), steps, "the step stays, to undo once the share is back");
    // nothing deleted, nothing half-moved, and the library still points at the share
    assert!(away.join("a.jpg").is_file() && away.join("a.xmp").is_file() && away.join("b.jpg").is_file());
    assert!(!dir.0.join("local/a.jpg").exists() && !dir.0.join("local/b.jpg").exists());
    assert_eq!(file_of(&s, a), dir.path("share/a.jpg"));
    assert_eq!(file_of(&s, b), dir.path("share/b.jpg"));
    // the share is back: the same Undo moves them home, sidecar and all
    std::fs::rename(&away, dir.0.join("share")).unwrap();
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(std::fs::read_to_string(dir.path("local/a.jpg")).unwrap(), "A");
    assert!(dir.0.join("local/a.xmp").is_file() && dir.0.join("local/b.jpg").is_file());
    assert_eq!(file_of(&s, a), dir.path("local/a.jpg"));
    assert_eq!(file_of(&s, b), dir.path("local/b.jpg"));
}

#[cfg(unix)]
#[test]
fn undo_from_a_read_only_share_is_refused_and_deletes_nothing() {
    use std::os::unix::fs::PermissionsExt;
    let dir = Scratch::new("readonly");
    let (mut s, a, _) = moved_to_share(&dir);
    let steps = s.undo.len();
    std::fs::set_permissions(dir.0.join("share"), std::fs::Permissions::from_mode(0o555)).unwrap();
    // running as root ignores permissions: nothing to check there
    let root = std::fs::write(dir.0.join("share/probe"), "x").is_ok();
    if !root {
        assert!(s.execute("edit.undo", &json!({})).is_err());
        assert_eq!(s.undo.len(), steps, "the step stays");
        assert!(dir.0.join("share/a.jpg").is_file() && dir.0.join("share/b.jpg").is_file(), "nothing deleted");
        assert!(!dir.0.join("local/a.jpg").exists(), "no second copy left behind");
        assert_eq!(file_of(&s, a), dir.path("share/a.jpg"));
    }
    std::fs::set_permissions(dir.0.join("share"), std::fs::Permissions::from_mode(0o755)).unwrap();
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(dir.0.join("local/a.jpg").is_file());
}

/// The model volume spells paths with `/`; the code under test joins them with the host's separator.
fn host(path: &str) -> String {
    path.replace('/', std::path::MAIN_SEPARATOR_STR)
}

fn model_session(fs_files: &[&str]) -> (Session, Vec<PhotoId>) {
    let mut s = Session::new();
    let ids = fs_files
        .iter()
        .map(|path| {
            let id = s.catalog.alloc_photo_id();
            let name = std::path::Path::new(path).file_name().unwrap().to_string_lossy().to_string();
            let p = Photo::new(id, Source::File { path: host(path) }, &name, "JPEG", 1, 1, "2026-01-01T00:00:00");
            s.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
            id
        })
        .collect();
    (s, ids)
}

fn plans(s: &Session, ids: &[PhotoId], into: &str) -> Vec<crate::rename::RenamePlan> {
    ids.iter()
        .map(|id| {
            let from = file_of(s, *id);
            let name = std::path::Path::new(&from).file_name().unwrap().to_string_lossy().to_string();
            crate::rename::RenamePlan {
                id: id.0,
                from: name.clone(),
                to: name.clone(),
                from_path: Some(from),
                to_path: Some(host(&format!("{into}/{name}"))),
            }
        })
        .collect()
}

/// Every photo points at a file the model volume holds (or, for `offline`, one it held).
fn assert_linked(s: &Session, fs: &crate::rename::tests::FakeFs, offline: &[&str]) {
    let files = fs.listing();
    for p in s.catalog.photos() {
        let Source::File { path } = &p.source else { continue };
        let path = path.replace('\\', "/");
        assert!(
            files.iter().any(|(f, _)| *f == path) || offline.contains(&path.as_str()),
            "photo {} points at {path}, which isn't there: {files:?}",
            p.id.0
        );
    }
}

#[test]
fn a_connection_lost_partway_through_a_move_puts_back_what_moved() {
    let fs = crate::rename::tests::FakeFs::new(false, &[("/local/a.jpg", "A"), ("/local/b.jpg", "B"), ("/local/c.jpg", "C")]);
    let (mut s, ids) = model_session(&["/local/a.jpg", "/local/b.jpg", "/local/c.jpg"]);
    let plans = plans(&s, &ids, "/share");
    // the share stops answering when the second file goes
    fs.fail.borrow_mut().push("/local/b.jpg".into());
    let steps = s.undo.len();
    assert!(s.apply_file_plans(&fs, &plans, &|_| "Move to Folder".into()).is_err());
    assert_eq!(
        fs.listing(),
        vec![("/local/a.jpg".into(), "A".into()), ("/local/b.jpg".into(), "B".into()), ("/local/c.jpg".into(), "C".into())],
        "all back home"
    );
    assert_eq!(s.undo.len(), steps, "no step for a move that didn't happen");
    assert_linked(&s, &fs, &[]);
}

#[test]
fn a_file_the_share_cannot_give_back_rolls_the_whole_undo_back() {
    let fs = crate::rename::tests::FakeFs::new(false, &[("/local/a.jpg", "A"), ("/local/b.jpg", "B")]);
    let (mut s, ids) = model_session(&["/local/a.jpg", "/local/b.jpg"]);
    let plans = plans(&s, &ids, "/share");
    s.apply_file_plans(&fs, &plans, &|_| "Move to Folder".into()).unwrap();
    assert_eq!(fs.listing(), vec![("/share/a.jpg".into(), "A".into()), ("/share/b.jpg".into(), "B".into())]);
    // b can't be read back: the share answers for a but not for b
    fs.fail.borrow_mut().push("/share/b.jpg".into());
    let e = s.undo_step_with(&fs).unwrap_err().to_string();
    assert!(e.contains("can't move the files back"), "{e}");
    // a came back before b failed: it went out again, so the step is whole and every photo
    // points at its file, none lost
    assert_linked(&s, &fs, &[]);
    let contents: Vec<String> = fs.listing().into_iter().map(|(_, c)| c).collect();
    assert_eq!(contents, vec!["A".to_string(), "B".to_string()], "both files still exist once: {:?}", fs.listing());
    // the share answers again: undoing whatever is left brings both home
    fs.fail.borrow_mut().clear();
    while s.undo_step_with(&fs).is_ok() && fs.listing().iter().any(|(p, _)| p.starts_with("/share")) {}
    assert_eq!(fs.listing(), vec![("/local/a.jpg".into(), "A".into()), ("/local/b.jpg".into(), "B".into())]);
    assert_linked(&s, &fs, &[]);
}

/// Where this machine has a second file system to write to (Linux's `/dev/shm`), a real move
/// across devices: copied, checked, the original removed, and Undo copies it back. Skipped where
/// there is none (a network share can't be mounted in a test).
#[test]
fn a_move_across_devices_copies_checks_and_removes_and_undoes_the_same_way() {
    let dir = Scratch::new("devices");
    let Some(other) = other_device(&dir.0) else { return };
    let share = other.join(format!("lc-share-other-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&share);
    std::fs::create_dir_all(&share).unwrap();
    let mut s = Session::new();
    let body: String = (0..300_000).map(|i| char::from(b'a' + (i % 26) as u8)).collect();
    let a = add_file(&mut s, &dir.path("local/a.jpg"), &body);
    let r = s.execute("photo.moveToFolder", &json!({"ids": [a.0], "folder": share.to_string_lossy()}));
    let moved =
        r.is_ok() && std::fs::read_to_string(share.join("a.jpg")).ok().as_deref() == Some(body.as_str()) && !dir.0.join("local/a.jpg").exists();
    let undone = moved
        && s.execute("edit.undo", &json!({})).is_ok()
        && std::fs::read_to_string(dir.0.join("local/a.jpg")).ok().as_deref() == Some(body.as_str());
    let gone_from_share = !share.join("a.jpg").exists();
    let _ = std::fs::remove_dir_all(&share);
    assert!(moved, "moved across devices: {r:?}");
    assert!(undone, "and back");
    assert!(gone_from_share, "no copy left on the other device");
}

/// Issue #4650: a folder moved to another volume, where a rename can't go, is copied whole
/// (subfolders, sidecars, hidden files, empty folders, file dates), checked, and only then
/// removed; Undo brings it back the same way.
#[test]
fn a_folder_moved_across_devices_is_copied_whole_then_removed_and_undoes() {
    let dir = Scratch::new("folder-devices");
    let Some(other) = other_device(&dir.0) else {
        lightcraft_testkit::skip!(System, "no second writable file system (/dev/shm) to move a folder to");
        return;
    };
    let share = Scratch(other.join(format!("lc-folder-other-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&share.0);
    std::fs::create_dir_all(&share.0).unwrap();
    let (trip, moved) = (dir.0.join("local/trip"), share.0.join("trip"));
    std::fs::create_dir_all(trip.join("day2")).unwrap();
    std::fs::create_dir_all(trip.join("empty")).unwrap();
    let mut s = Session::new();
    let body: String = (0..300_000).map(|i| char::from(b'a' + (i % 26) as u8)).collect();
    let a = add_file(&mut s, &dir.path("local/trip/a.jpg"), &body);
    let b = add_file(&mut s, &dir.path("local/trip/day2/b.jpg"), "B");
    std::fs::write(trip.join("a.xmp"), "<edits/>").unwrap();
    std::fs::write(trip.join(".hidden"), "h").unwrap();
    // a sidecar's date says whether it changed since import: the copy keeps it
    let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_600_000_000);
    std::fs::File::options().write(true).open(trip.join("a.xmp")).unwrap().set_modified(old).unwrap();

    let r = s.execute("folder.move", &json!({"path": trip.to_string_lossy(), "into": share.0.to_string_lossy()})).unwrap();
    assert_eq!(r["relinked"], 2, "{r}");
    assert!(!trip.exists(), "the original folder is gone once the copy is checked");
    assert_eq!(std::fs::read_to_string(moved.join("a.jpg")).unwrap(), body);
    assert_eq!(std::fs::read_to_string(moved.join("day2/b.jpg")).unwrap(), "B");
    assert_eq!(std::fs::read_to_string(moved.join("a.xmp")).unwrap(), "<edits/>");
    assert_eq!(std::fs::read_to_string(moved.join(".hidden")).unwrap(), "h");
    assert!(moved.join("empty").is_dir(), "an empty folder goes along");
    assert_eq!(std::fs::metadata(moved.join("a.xmp")).unwrap().modified().unwrap(), old, "the file date is kept");
    let names: Vec<_> = std::fs::read_dir(&moved).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
    assert!(!names.iter().any(|n| n.contains("lc-partial")), "no partial copy left: {names:?}");
    assert!(file_of(&s, a).starts_with(&moved.to_string_lossy().to_string()) && file_of(&s, b).ends_with("day2/b.jpg"));

    s.execute("edit.undo", &json!({})).unwrap();
    assert!(!moved.exists(), "Undo leaves no copy on the other device");
    assert_eq!(std::fs::read_to_string(trip.join("a.jpg")).unwrap(), body);
    assert_eq!(std::fs::read_to_string(trip.join("day2/b.jpg")).unwrap(), "B");
    assert!(trip.join("empty").is_dir());
    assert_eq!(file_of(&s, a), dir.path("local/trip/a.jpg"));
}

/// Files the old place won't give up after a complete, checked copy to another volume: the move
/// stands (the photos follow the copy), and `folder.move` and `edit.undo` name each file left
/// behind in `warnings` instead of reporting a clean move.
#[cfg(unix)]
#[test]
fn a_folder_move_that_leaves_files_behind_names_them() {
    use std::os::unix::fs::PermissionsExt;
    let dir = Scratch::new("folder-left-behind");
    let Some(other) = other_device(&dir.0) else {
        lightcraft_testkit::skip!(System, "no second writable file system (/dev/shm) to move a folder to");
        return;
    };
    let share = Scratch(other.join(format!("lc-folder-left-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&share.0);
    std::fs::create_dir_all(&share.0).unwrap();
    let (trip, moved) = (dir.0.join("local/trip"), share.0.join("trip"));
    std::fs::create_dir_all(trip.join("day2")).unwrap();
    let mut s = Session::new();
    // a file outside the read-only folder goes first, so the removal is already under way
    let a = add_file(&mut s, &dir.path("local/trip/a.jpg"), "A");
    let b = add_file(&mut s, &dir.path("local/trip/day2/b.jpg"), "B");
    let lock = |p: &std::path::Path, mode| std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).unwrap();
    lock(&trip.join("day2"), 0o555);
    if std::fs::write(trip.join("day2/.probe"), b"x").is_ok() {
        lock(&trip.join("day2"), 0o755);
        lightcraft_testkit::skip!(System, "running with rights that ignore a read-only folder");
        return;
    }
    let r = s.execute("folder.move", &json!({"path": trip.to_string_lossy(), "into": share.0.to_string_lossy()}));
    lock(&trip.join("day2"), 0o755);
    let r = r.unwrap();
    let warned = |r: &serde_json::Value, file: &std::path::Path| {
        r["warnings"].as_array().is_some_and(|w| w.iter().any(|w| w.as_str().is_some_and(|w| w.contains(&*file.to_string_lossy()))))
    };
    assert!(warned(&r, &trip.join("day2/b.jpg")), "the file left behind is named: {r}");
    assert!(!trip.join("a.jpg").exists(), "what could be removed was");
    assert_eq!(std::fs::read_to_string(moved.join("day2/b.jpg")).unwrap(), "B", "the copy is whole");
    assert!(file_of(&s, a).starts_with(&*moved.to_string_lossy()) && file_of(&s, b).starts_with(&*moved.to_string_lossy()));

    // once the old place is cleared, Undo moves the folder back; this time the copy's side holds on
    std::fs::remove_dir_all(&trip).unwrap();
    lock(&moved.join("day2"), 0o555);
    let r = s.execute("edit.undo", &json!({}));
    lock(&moved.join("day2"), 0o755);
    let r = r.unwrap();
    assert!(warned(&r, &moved.join("day2/b.jpg")), "Undo names the file left behind too: {r}");
    assert_eq!(std::fs::read_to_string(trip.join("day2/b.jpg")).unwrap(), "B");
    assert_eq!(file_of(&s, b), dir.path("local/trip/day2/b.jpg"));
}

/// A writable folder on another device than `here`, if there is one.
fn other_device(here: &std::path::Path) -> Option<std::path::PathBuf> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let dev = std::fs::metadata(here).ok()?.dev();
        ["/dev/shm", "/run/shm"]
            .iter()
            .map(std::path::PathBuf::from)
            .find(|p| {
                std::fs::metadata(p).is_ok_and(|m| m.dev() != dev)
                    && std::fs::write(p.join(format!(".lc-probe-{}", std::process::id())), b"x").is_ok()
            })
            .inspect(|p| {
                let _ = std::fs::remove_file(p.join(format!(".lc-probe-{}", std::process::id())));
            })
    }
    #[cfg(not(unix))]
    {
        let _ = here;
        None
    }
}

#[cfg(unix)]
#[test]
fn a_link_is_not_copied_as_its_target() {
    use crate::rename::MoveFs;
    let dir = Scratch::new("copy-link");
    std::fs::create_dir_all(dir.0.join("local")).unwrap();
    std::fs::create_dir_all(dir.0.join("share")).unwrap();
    std::fs::write(dir.0.join("local/private.txt"), b"secret").unwrap();
    std::os::unix::fs::symlink(dir.0.join("local/private.txt"), dir.0.join("local/photo.xmp")).unwrap();
    assert!(crate::rename::RealFs.copy_no_replace(&dir.0.join("local/photo.xmp"), &dir.0.join("share/photo.xmp")).is_err());
    assert!(!dir.0.join("share/photo.xmp").exists());
}

#[test]
fn the_copy_across_volumes_is_whole_or_absent_and_never_replaces() {
    use crate::rename::MoveFs;
    let dir = Scratch::new("copy");
    let body: Vec<u8> = (0..(3usize << 20) + 17).map(|i| (i % 251) as u8).collect();
    let (from, to) = (dir.0.join("local/big.raw"), dir.0.join("share/big.raw"));
    std::fs::write(&from, &body).unwrap();
    crate::rename::RealFs.copy_no_replace(&from, &to).unwrap();
    assert_eq!(std::fs::read(&to).unwrap(), body);
    // nothing left under a hidden name beside it
    let left: Vec<_> = std::fs::read_dir(dir.0.join("share")).unwrap().flatten().map(|e| e.file_name()).collect();
    assert_eq!(left, vec![std::ffi::OsString::from("big.raw")], "{left:?}");
    // a partial left by a copy the share cut off doesn't block the next copy
    let stale = dir.0.join(format!("share/.again.raw.lc-partial-{}-0", std::process::id()));
    std::fs::write(&stale, b"cut off").unwrap();
    crate::rename::RealFs.copy_no_replace(&from, &dir.0.join("share/again.raw")).unwrap();
    assert_eq!(std::fs::read(dir.0.join("share/again.raw")).unwrap(), body);
    std::fs::remove_file(&stale).unwrap();
    std::fs::remove_file(dir.0.join("share/again.raw")).unwrap();
    // a copy that can't start (its source unreadable) leaves nothing at all under the photo's name
    let gone = dir.0.join("local/missing.raw");
    assert!(crate::rename::RealFs.copy_no_replace(&gone, &dir.0.join("share/missing.raw")).is_err());
    assert!(!dir.0.join("share/missing.raw").exists());
    let left: Vec<_> = std::fs::read_dir(dir.0.join("share")).unwrap().flatten().map(|e| e.file_name()).collect();
    assert_eq!(left.len(), 1, "no partial file left: {left:?}");
    // a name taken is never replaced, and what is there stays
    std::fs::write(&to, b"other").unwrap();
    assert!(crate::rename::RealFs.copy_no_replace(&from, &to).is_err());
    assert_eq!(std::fs::read(&to).unwrap(), b"other");
}

/// A folder copied to another volume with a link inside would copy the link's target as a new,
/// readable file: the move is refused before anything is copied, and the folder stays whole.
#[cfg(unix)]
#[test]
fn a_folder_with_a_link_inside_is_not_copied_across_volumes() {
    let dir = Scratch::new("folder-link");
    std::fs::create_dir_all(dir.0.join("local/trip/day2")).unwrap();
    std::fs::write(dir.0.join("local/trip/a.jpg"), b"A").unwrap();
    std::fs::write(dir.0.join("local/private.txt"), b"secret").unwrap();
    std::os::unix::fs::symlink(dir.0.join("local/private.txt"), dir.0.join("local/trip/day2/b.xmp")).unwrap();
    let e = crate::rename::move_folder_across_volumes(&dir.0.join("local/trip"), &dir.0.join("share/trip")).unwrap_err();
    assert!(e.to_string().contains("b.xmp"), "names what refused it: {e}");
    assert!(!dir.0.join("share/trip").exists(), "nothing copied");
    assert!(dir.0.join("local/trip/a.jpg").is_file() && dir.0.join("local/trip/day2/b.xmp").is_symlink());
}

/// Originals that can't be removed (a read-only volume) leave the folder where it was, with no
/// second copy on the other side.
#[cfg(unix)]
#[test]
fn a_folder_that_cannot_be_removed_after_copying_stays_and_the_copy_goes() {
    use std::os::unix::fs::PermissionsExt;
    let dir = Scratch::new("folder-readonly");
    let trip = dir.0.join("local/trip");
    std::fs::create_dir_all(&trip).unwrap();
    std::fs::write(trip.join("a.jpg"), b"A").unwrap();
    std::fs::set_permissions(&trip, std::fs::Permissions::from_mode(0o555)).unwrap();
    if std::fs::write(trip.join(".probe"), b"x").is_ok() {
        std::fs::set_permissions(&trip, std::fs::Permissions::from_mode(0o755)).unwrap();
        lightcraft_testkit::skip!(System, "running with rights that ignore a read-only folder");
        return;
    }
    let r = crate::rename::move_folder_across_volumes(&trip, &dir.0.join("share/trip"));
    std::fs::set_permissions(&trip, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(r.is_err(), "{r:?}");
    assert_eq!(std::fs::read(trip.join("a.jpg")).unwrap(), b"A");
    assert!(!dir.0.join("share/trip").exists(), "the copy is removed");
}
