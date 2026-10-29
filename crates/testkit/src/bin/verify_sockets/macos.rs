//! The experiment itself: macOS only.

use std::ffi::{CString, c_char, c_int};
use std::os::fd::FromRawFd;
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

unsafe extern "C" {
    fn launch_activate_socket(name: *const c_char, fds: *mut *mut c_int, cnt: *mut usize) -> c_int;
}

fn activate(name: &str) -> Result<Vec<c_int>, i32> {
    let c = CString::new(name).unwrap();
    let mut fds: *mut c_int = std::ptr::null_mut();
    let mut cnt: usize = 0;
    let r = unsafe { launch_activate_socket(c.as_ptr(), &mut fds, &mut cnt) };
    if r != 0 {
        return Err(r);
    }
    let v = unsafe { std::slice::from_raw_parts(fds, cnt) }.to_vec();
    unsafe { libc::free(fds as *mut libc::c_void) };
    Ok(v)
}

async fn serve(
    name: &'static str,
    l: tokio::net::TcpListener,
    tls: Option<tokio_rustls::TlsAcceptor>,
) {
    loop {
        let Ok((mut s, peer)) = l.accept().await else {
            continue;
        };
        let tls = tls.clone();
        tokio::spawn(async move {
            let body = format!("ok {name} from {peer}\n");
            let resp = format!(
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let mut buf = [0u8; 2048];
            if let Some(acc) = tls {
                let Ok(mut t) = acc.accept(s).await else {
                    return;
                };
                let _ = t.read(&mut buf).await;
                let _ = t.write_all(resp.as_bytes()).await;
                let _ = t.shutdown().await;
            } else {
                let _ = s.read(&mut buf).await;
                let _ = s.write_all(resp.as_bytes()).await;
            }
        });
    }
}

#[tokio::main]
pub async fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("bind") => {
            let addr = args.get(2).expect("addr");
            match std::net::TcpListener::bind(addr) {
                Ok(l) => println!("bind {addr}: OK ({:?})", l.local_addr()),
                Err(e) => println!("bind {addr}: FAILED {e}"),
            }
        }
        Some("serve") => {
            let _ = rustls::crypto::ring::default_provider().install_default();
            let ck = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
            let key =
                rustls::pki_types::PrivateKeyDer::Pkcs8(ck.signing_key.serialize_der().into());
            let cfg = rustls::ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(vec![ck.cert.der().clone()], key)
                .unwrap();
            let acc = tokio_rustls::TlsAcceptor::from(Arc::new(cfg));
            for name in ["https4", "https6", "http4", "http6"] {
                match activate(name) {
                    Ok(fds) => {
                        eprintln!("activate {name}: {} fd(s) {fds:?}", fds.len());
                        for fd in fds {
                            let std_l = unsafe { std::net::TcpListener::from_raw_fd(fd) };
                            eprintln!("  {name} fd {fd} local {:?}", std_l.local_addr());
                            std_l.set_nonblocking(true).unwrap();
                            let l = tokio::net::TcpListener::from_std(std_l).unwrap();
                            let tls = name.starts_with("https").then(|| acc.clone());
                            tokio::spawn(serve(name, l, tls));
                        }
                    }
                    Err(e) => eprintln!("activate {name}: error {e}"),
                }
            }
            std::future::pending::<()>().await;
        }
        _ => eprintln!("usage: verify_sockets serve | bind <addr>"),
    }
}
