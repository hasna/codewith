use std::io;
use std::path::Path;

/// Atomically moves `source` to `destination` without replacing an existing path.
///
/// The platform primitive performs the existence check and rename as one filesystem operation, so
/// another process cannot introduce a destination between a userspace check and the move.
pub(super) fn move_file_no_replace(source: &Path, destination: &Path) -> io::Result<()> {
    platform::move_file_no_replace(source, destination)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod platform {
    use std::io;
    use std::path::Path;

    use rustix::fs::CWD;
    use rustix::fs::RenameFlags;
    use rustix::fs::renameat_with;

    pub(super) fn move_file_no_replace(source: &Path, destination: &Path) -> io::Result<()> {
        renameat_with(CWD, source, CWD, destination, RenameFlags::NOREPLACE)
            .map_err(|err| io::Error::from_raw_os_error(err.raw_os_error()))
    }
}

#[cfg(windows)]
mod platform {
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use windows_sys::Win32::Storage::FileSystem::MoveFileExW;

    pub(super) fn move_file_no_replace(source: &Path, destination: &Path) -> io::Result<()> {
        let source = wide_path(source)?;
        let destination = wide_path(destination)?;
        // SAFETY: both path pointers are valid NUL-terminated UTF-16 strings for the duration of
        // the call. Omitting `MOVEFILE_REPLACE_EXISTING` makes `MoveFileExW` reject an existing
        // destination.
        let result = unsafe {
            MoveFileExW(source.as_ptr(), destination.as_ptr(), /*dwflags*/ 0)
        };
        if result != 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    fn wide_path(path: &Path) -> io::Result<Vec<u16>> {
        let mut wide = path.as_os_str().encode_wide().collect::<Vec<_>>();
        if wide.contains(&0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "filesystem path contains an interior NUL character",
            ));
        }
        wide.push(0);
        Ok(wide)
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod platform {
    use std::io;
    use std::path::Path;

    pub(super) fn move_file_no_replace(_source: &Path, _destination: &Path) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "atomic no-replace file moves are unsupported on this platform",
        ))
    }
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos", windows)))]
mod tests {
    use std::sync::Barrier;

    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn move_file_no_replace_moves_source() {
        let temp_dir = TempDir::new().expect("temp dir");
        let source = temp_dir.path().join("source");
        let destination = temp_dir.path().join("destination");
        std::fs::write(&source, b"source rollout").expect("write source");

        move_file_no_replace(&source, &destination).expect("move source");

        assert!(!source.exists());
        assert_eq!(
            std::fs::read(&destination).expect("destination contents"),
            b"source rollout"
        );
    }

    #[test]
    fn move_file_no_replace_preserves_existing_destination() {
        let temp_dir = TempDir::new().expect("temp dir");
        let source = temp_dir.path().join("source");
        let destination = temp_dir.path().join("destination");
        std::fs::write(&source, b"source rollout").expect("write source");
        std::fs::write(&destination, b"destination rollout").expect("write destination");

        let err = move_file_no_replace(&source, &destination)
            .expect_err("existing destination must reject the move");

        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert!(err.raw_os_error().is_some());
        assert_eq!(
            std::fs::read(&source).expect("source remains"),
            b"source rollout"
        );
        assert_eq!(
            std::fs::read(&destination).expect("destination remains"),
            b"destination rollout"
        );
    }

    #[test]
    fn concurrent_moves_publish_exactly_one_source() {
        let temp_dir = TempDir::new().expect("temp dir");
        let sources = [
            temp_dir.path().join("source-one"),
            temp_dir.path().join("source-two"),
        ];
        let contents = [b"first rollout".as_slice(), b"second rollout".as_slice()];
        let destination = temp_dir.path().join("destination");
        std::fs::write(&sources[0], contents[0]).expect("write first source");
        std::fs::write(&sources[1], contents[1]).expect("write second source");
        let barrier = Barrier::new(3);

        let results = std::thread::scope(|scope| {
            let barrier = &barrier;
            let first_source = sources[0].clone();
            let first_destination = destination.clone();
            let first = scope.spawn(move || {
                barrier.wait();
                move_file_no_replace(&first_source, &first_destination)
            });
            let second_source = sources[1].clone();
            let second_destination = destination.clone();
            let second = scope.spawn(move || {
                barrier.wait();
                move_file_no_replace(&second_source, &second_destination)
            });
            barrier.wait();
            [
                first.join().expect("first move thread"),
                second.join().expect("second move thread"),
            ]
        });

        let successful_moves = results.iter().filter(|result| result.is_ok()).count();
        assert_eq!(successful_moves, 1);
        let winner = results
            .iter()
            .position(Result::is_ok)
            .expect("one move succeeds");
        let loser = 1 - winner;
        assert!(!sources[winner].exists());
        assert_eq!(
            std::fs::read(&sources[loser]).expect("losing source remains"),
            contents[loser]
        );
        assert_eq!(
            std::fs::read(&destination).expect("destination contents"),
            contents[winner]
        );
        assert_eq!(
            results[loser]
                .as_ref()
                .expect_err("losing move fails")
                .kind(),
            io::ErrorKind::AlreadyExists
        );
    }
}
