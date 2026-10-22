//! A socket named relative to the working directory is checked in the
//! directory it resolves in. This binary holds one test because it changes
//! the process's working directory, which every other test would share.

use std::os::unix::fs::PermissionsExt;

use fabric_client::{Client, ClientError};

/// `Client::new("fabric.sock")` names a socket in the working directory.
/// Its parent as written is the empty path, which does not exist, so the
/// check that the directory is the user's own with mode 0700 used to pass
/// without looking (review finding on PR #13). A working directory others
/// may enter is refused before anything is sent, as the TypeScript SDK
/// refuses it.
#[tokio::test]
async fn a_bare_socket_name_is_checked_in_the_working_directory() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::env::set_current_dir(dir.path()).unwrap();
    let got = Client::new("fabric.sock").hello().await;
    let Err(ClientError::Untrusted { path, .. }) = got else {
        panic!("{got:?}");
    };
    assert_eq!(
        path.canonicalize().unwrap(),
        dir.path().canonicalize().unwrap()
    );

    // With the directory private, the check passes and the connect reports that no node listens.
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let got = Client::new("fabric.sock").hello().await;
    assert!(matches!(got, Err(ClientError::NoDaemon { .. })), "{got:?}");
}
