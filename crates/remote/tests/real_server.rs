//! Against a real SFTP server - only when `DNNM_SFTP_TEST` names one, as `host:port:user:password:folder` (a folder
//! the user may write in, which the test empties of what it made). With Docker:
//!
//! ```text
//! docker run -d --name dnnm-sftp-test -p 2222:22 atmoz/sftp tester:secret:::upload
//! DNNM_SFTP_TEST=127.0.0.1:2222:tester:secret:/upload cargo test -p dnnm-remote --test real_server
//! ```

use std::sync::Arc;

use dnnm_application::operation::CancelToken;
use dnnm_application::source_control::model::{Auth, Environment, EnvironmentKind};
use dnnm_application::source_control::sync::{Remote, RemoteError};
use dnnm_remote::Servers as Sftp;

fn server() -> Option<(Environment, String)> {
    let spec = std::env::var("DNNM_SFTP_TEST").ok()?;
    let parts: Vec<&str> = spec.splitn(5, ':').collect();
    let [host, port, user, password, folder] = parts[..] else { panic!("DNNM_SFTP_TEST: host:port:user:password:folder") };
    let env = Environment {
        id: "real-server-test".into(),
        project: "test".into(),
        name: "Test".into(),
        kind: EnvironmentKind::Test,
        protocol: dnnm_application::source_control::model::Protocol::Sftp,
        host: host.into(),
        port: port.parse().unwrap(),
        user: user.into(),
        auth: Auth::Password,
        remote_root: folder.into(),
        mappings: Vec::new(),
        ignore: Vec::new(),
        protected: Vec::new(),
        backup_remote: false,
        host_key: None,
    };
    Some((env, password.to_string()))
}

#[test]
fn upload_list_download_replace_and_remove() {
    let Some((mut env, password)) = server() else {
        eprintln!("DNNM_SFTP_TEST isn't set - skipped");
        return;
    };
    let sftp = Sftp::new(Arc::new(move |_| Some(password.clone())));
    let key = sftp.host_key(&env).unwrap();
    assert!(key.starts_with("SHA256:"), "{key}");

    env.host_key = Some("SHA256:not-this-one".into());
    assert!(
        matches!(sftp.connect(&env), Err(RemoteError::Other(e)) if e.contains("isn't the one trusted")),
        "a different key is refused"
    );
    env.host_key = Some(key);
    let mut fs = sftp.connect(&env).unwrap();
    let cancel = CancelToken::new();
    let root = env.remote_root.trim_end_matches('/');
    let file = format!("{root}/dnnm-test/Portals/0/skin.css");

    // Larger than one chunk, so the streaming is tried.
    let big: Vec<u8> = (0..200_000u32).flat_map(|i| i.to_le_bytes()).collect();
    fs.upload(&mut big.as_slice(), &file, 1_700_000_000, &mut |_| {}, &cancel).unwrap();
    let meta = fs.stat(&file).unwrap().unwrap();
    assert_eq!(meta.size, big.len() as u64);
    assert_eq!(meta.mtime, 1_700_000_000, "the time was kept");

    let listed = fs.list(&format!("{root}/dnnm-test"), &cancel).unwrap();
    assert_eq!(listed.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>(), ["Portals/0/skin.css"], "no part file left");

    let mut back = Vec::new();
    fs.download(&file, &mut back, &mut |_| {}, &cancel).unwrap();
    assert_eq!(back, big);

    // Replaced in place.
    fs.upload(&mut b"body{}".as_slice(), &file, 1_700_000_100, &mut |_| {}, &cancel).unwrap();
    let mut small = Vec::new();
    fs.download(&file, &mut small, &mut |_| {}, &cancel).unwrap();
    assert_eq!(small, b"body{}");

    fs.remove(&file).unwrap();
    assert_eq!(fs.stat(&file).unwrap(), None);
    fs.remove(&file).unwrap();
    assert!(fs.list(&format!("{root}/not-made-yet"), &cancel).unwrap().is_empty());
}
