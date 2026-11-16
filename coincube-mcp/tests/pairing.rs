#![cfg(all(any(unix, windows), feature = "adapter"))]
use coincube_mcp::{
    adapter::Adapter,
    ipc::{Grant, Server},
    pairing, *,
};
use std::{
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

struct Fixture;
impl Reader for Fixture {
    fn read(
        &self,
        _: Request,
    ) -> Pin<Box<dyn Future<Output = Result<Response, Error>> + Send + '_>> {
        Box::pin(async { Ok(Response::List(WalletList { wallets: vec![] })) })
    }
}
fn directory() -> (tempfile::TempDir, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().canonicalize().unwrap().join("pairing");
    std::fs::create_dir(&dir).unwrap();
    pairing::protect_new(&dir, true).unwrap();
    (root, dir)
}

#[tokio::test]
async fn same_running_adapter_reconnects_after_lock_and_server_replacement() {
    let (_root, dir) = directory();
    let locator = dir.join("fixture.endpoint");
    let credential = "ab".repeat(32);
    let session = Arc::new(AtomicU64::new(1));
    let make = |generation| {
        let session = session.clone();
        Grant::paired(
            "wallet".into(),
            [true, false, false],
            credential.clone(),
            move || session.load(Ordering::SeqCst) == generation,
        )
        .unwrap()
    };
    let first = Server::start(Arc::new(Fixture), make(1)).unwrap();
    pairing::publish(&locator, &first.socket).unwrap();
    let adapter = Adapter {
        socket: PathBuf::new(),
        credential: credential.clone(),
        pairing: Some(locator.clone()),
    };
    assert_ne!(
        adapter
            .invoke("tenshu_list_wallets", serde_json::json!({}))
            .await
            .unwrap()
            .is_error,
        Some(true)
    );
    session.store(2, Ordering::SeqCst);
    assert_eq!(
        adapter
            .invoke("tenshu_list_wallets", serde_json::json!({}))
            .await
            .unwrap()
            .structured_content
            .unwrap()["error"]["code"],
        "access_denied"
    );
    drop(first);
    assert_eq!(
        adapter
            .invoke("tenshu_list_wallets", serde_json::json!({}))
            .await
            .unwrap()
            .structured_content
            .unwrap()["error"]["code"],
        "wallet_unavailable"
    );
    let second = Server::start(Arc::new(Fixture), make(2)).unwrap();
    pairing::publish(&locator, &second.socket).unwrap();
    assert_ne!(
        adapter
            .invoke("tenshu_list_wallets", serde_json::json!({}))
            .await
            .unwrap()
            .is_error,
        Some(true)
    );
    let wrong = Adapter {
        socket: PathBuf::new(),
        credential: "cd".repeat(32),
        pairing: Some(locator.clone()),
    };
    assert_eq!(
        wrong
            .invoke("tenshu_list_wallets", serde_json::json!({}))
            .await
            .unwrap()
            .structured_content
            .unwrap()["error"]["code"],
        "access_denied"
    );
    second.grant.revoke();
    assert_eq!(
        adapter
            .invoke("tenshu_list_wallets", serde_json::json!({}))
            .await
            .unwrap()
            .structured_content
            .unwrap()["error"]["code"],
        "access_denied"
    );
}

#[test]
fn locator_rejects_corruption_links_and_shared_permissions() {
    let (_root, dir) = directory();
    let locator = dir.join("fixture.endpoint");
    pairing::write_private(&locator, b"not JSON").unwrap();
    assert!(pairing::resolve(&locator).is_err());
    pairing::publish(
        &locator,
        std::path::Path::new("/tmp/coincube-mcp-fixture/rpc"),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::{PermissionsExt, symlink};
        std::fs::set_permissions(&locator, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(pairing::resolve(&locator).is_err());
        std::fs::remove_file(&locator).unwrap();
        let target = dir.join("target");
        std::fs::write(&target, b"{}").unwrap();
        symlink(&target, &locator).unwrap();
        assert!(pairing::resolve(&locator).is_err());
    }
}

#[tokio::test]
async fn paired_runtime_keeps_pre_and_post_read_session_checks() {
    struct Slow {
        entered: Arc<tokio::sync::Notify>,
        finish: Arc<tokio::sync::Notify>,
    }
    impl Reader for Slow {
        fn read(
            &self,
            _: Request,
        ) -> Pin<Box<dyn Future<Output = Result<Response, Error>> + Send + '_>> {
            Box::pin(async move {
                self.entered.notify_one();
                self.finish.notified().await;
                Ok(Response::List(WalletList { wallets: vec![] }))
            })
        }
    }
    let active = Arc::new(AtomicU64::new(1));
    let guarded = active.clone();
    let credential = "ab".repeat(32);
    let entered = Arc::new(tokio::sync::Notify::new());
    let finish = Arc::new(tokio::sync::Notify::new());
    let server = Server::start(
        Arc::new(Slow {
            entered: entered.clone(),
            finish: finish.clone(),
        }),
        Grant::paired("wallet".into(), [true; 3], credential.clone(), move || {
            guarded.load(Ordering::SeqCst) == 1
        })
        .unwrap(),
    )
    .unwrap();
    let socket = server.socket.clone();
    let call =
        tokio::spawn(async move { ipc::call(&socket, &credential, Request::List(Empty {})).await });
    entered.notified().await;
    active.store(2, Ordering::SeqCst);
    finish.notify_one();
    assert_eq!(call.await.unwrap().unwrap_err(), Error::AccessDenied);
    assert!(!server.grant.active());
}

#[cfg(windows)]
#[test]
fn windows_atomic_replacement_retains_private_owner_acl() {
    let (_root, dir) = directory();
    let path = dir.join("registry");
    for value in [b"first".as_slice(), b"replacement".as_slice()] {
        pairing::write_private(&path, value).unwrap();
        pairing::check_private(&path, false).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), value);
    }
    // A default inherited ACL must not be accepted as a private registry.
    let inherited = dir.parent().unwrap().join("inherited-file");
    std::fs::write(&inherited, b"not private").unwrap();
    assert!(pairing::check_private(&inherited, false).is_err());
}
