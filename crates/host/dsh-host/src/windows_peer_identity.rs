//! Authenticate local control-plane TCP peers by their OS principal.
//! A sandbox account must not obtain the host user's authority through HTTP.
use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Arc,
};
use windows_sys::Win32::{
    Foundation::CloseHandle,
    NetworkManagement::IpHelper::{
        GetExtendedTcpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCPROW_OWNER_PID, TCP_TABLE_OWNER_PID_ALL,
    },
    Security::{GetLengthSid, GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser},
    System::Threading::{
        GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
    },
};

struct Handle(windows_sys::Win32::Foundation::HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

fn principal(process: windows_sys::Win32::Foundation::HANDLE) -> Option<Vec<u8>> {
    let mut raw = std::ptr::null_mut();
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut raw) } == 0 {
        return None;
    }
    let token = Handle(raw);
    let mut bytes = 0;
    unsafe {
        GetTokenInformation(token.0, TokenUser, std::ptr::null_mut(), 0, &mut bytes);
    }
    if bytes == 0 || bytes > 64 * 1024 {
        return None;
    }
    let mut data = vec![0u64; (bytes as usize).div_ceil(8)];
    if unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            data.as_mut_ptr().cast(),
            bytes,
            &mut bytes,
        )
    } == 0
    {
        return None;
    }
    let user = unsafe { std::ptr::read_unaligned(data.as_ptr().cast::<TOKEN_USER>()) };
    let length = unsafe { GetLengthSid(user.User.Sid) } as usize;
    if length == 0 || length > 256 {
        return None;
    }
    Some(unsafe { std::slice::from_raw_parts(user.User.Sid.cast::<u8>(), length) }.to_vec())
}

fn owner(peer: SocketAddr, server: SocketAddr) -> Option<u32> {
    let family = if peer.is_ipv4() { 2 } else { 23 };
    let mut bytes = 0;
    unsafe {
        GetExtendedTcpTable(
            std::ptr::null_mut(),
            &mut bytes,
            0,
            family,
            TCP_TABLE_OWNER_PID_ALL,
            0,
        );
    }
    if bytes < 4 || bytes > 16 * 1024 * 1024 {
        return None;
    }
    let mut data = vec![0u64; (bytes as usize).div_ceil(8)];
    if unsafe {
        GetExtendedTcpTable(
            data.as_mut_ptr().cast(),
            &mut bytes,
            0,
            family,
            TCP_TABLE_OWNER_PID_ALL,
            0,
        )
    } != 0
    {
        return None;
    }
    let base = data.as_ptr().cast::<u8>();
    let count = unsafe { std::ptr::read_unaligned(base.cast::<u32>()) } as usize;
    let row_size = if family == 2 {
        std::mem::size_of::<MIB_TCPROW_OWNER_PID>()
    } else {
        std::mem::size_of::<MIB_TCP6ROW_OWNER_PID>()
    };
    if count > (bytes as usize - 4) / row_size {
        return None;
    }
    for index in 0..count {
        let row = unsafe { base.add(4 + index * row_size) };
        let (local, remote, pid) = if family == 2 {
            let row = unsafe { std::ptr::read_unaligned(row.cast::<MIB_TCPROW_OWNER_PID>()) };
            (
                SocketAddr::new(
                    IpAddr::V4(Ipv4Addr::from(row.dwLocalAddr.to_ne_bytes())),
                    u16::from_be(row.dwLocalPort as u16),
                ),
                SocketAddr::new(
                    IpAddr::V4(Ipv4Addr::from(row.dwRemoteAddr.to_ne_bytes())),
                    u16::from_be(row.dwRemotePort as u16),
                ),
                row.dwOwningPid,
            )
        } else {
            let row = unsafe { std::ptr::read_unaligned(row.cast::<MIB_TCP6ROW_OWNER_PID>()) };
            (
                SocketAddr::new(
                    IpAddr::V6(Ipv6Addr::from(row.ucLocalAddr)),
                    u16::from_be(row.dwLocalPort as u16),
                ),
                SocketAddr::new(
                    IpAddr::V6(Ipv6Addr::from(row.ucRemoteAddr)),
                    u16::from_be(row.dwRemotePort as u16),
                ),
                row.dwOwningPid,
            )
        };
        if local.ip() == peer.ip()
            && local.port() == peer.port()
            && remote.ip() == server.ip()
            && remote.port() == server.port()
        {
            return Some(pid);
        }
    }
    None
}

pub(super) fn filter() -> Result<dsh_host_webserver::ConnectionFilter, String> {
    let own = principal(unsafe { GetCurrentProcess() })
        .ok_or("cannot determine host principal for native sandbox control-plane isolation")?;
    Ok(Arc::new(move |peer, server| {
        let Some(pid) = owner(peer, server) else {
            return false;
        };
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if process.is_null() {
            return false;
        }
        let process = Handle(process);
        principal(process.0).is_some_and(|sid| sid == own)
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires explicitly provisioned native acceptance helpers"]
    async fn native_backend_runs_real_host_environment_probes() {
        use sha2::{Digest, Sha256};
        let binary =
            std::path::PathBuf::from(std::env::var("DSH_NATIVE_ACCEPTANCE_BINARY").unwrap());
        let state = std::env::var("DSH_NATIVE_ACCEPTANCE_HOME").unwrap();
        let workspace = std::env::var("DSH_NATIVE_ACCEPTANCE_WORKSPACE").unwrap();
        let directory =
            std::path::PathBuf::from(std::env::var("DSH_NATIVE_ACCEPTANCE_HOST_HOME").unwrap());
        std::fs::create_dir_all(&directory).unwrap();
        let digest =
            |path: &std::path::Path| format!("{:x}", Sha256::digest(std::fs::read(path).unwrap()));
        let parent = binary.parent().unwrap();
        let config = serde_json::json!({"version":1,"backend":"windows-native","runner":binary,"stateDirectory":state,"sha256":digest(&binary),"commandRunnerSha256":digest(&parent.join("dsh-command-runner.exe")),"setupSha256":digest(&parent.join("dsh-windows-sandbox-setup.exe")),"workspaces":[workspace]});
        std::fs::write(
            directory.join("windows-sandbox.json"),
            serde_json::to_vec(&config).unwrap(),
        )
        .unwrap();
        let ctx = cordis::Context::root();
        let host = crate::compose_persistent_host_at(&ctx, &directory, None).unwrap();
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let base = format!("http://127.0.0.1:{}", host.web_server.port());
        let created:serde_json::Value=client.post(format!("{base}/api/session.create")).json(&serde_json::json!({"type":"client-request","rpcId":"native-probe","method":"session.create","payload":{"cwd":workspace,"agentPreset":"standard"}})).send().await.unwrap().json().await.unwrap();
        assert_eq!(created["result"]["ok"], true, "{created}");
        let id = created["result"]["value"]["sessionId"].as_str().unwrap();
        let mut results = Vec::new();
        for name in ["shell", "node", "python", "cargo", "rustc", "git"] {
            let result:serde_json::Value=client.post(format!("{base}/__dsh-environment")).json(&serde_json::json!({"action":"probe","name":name,"level":"launch","sessionId":id,"refresh":true})).send().await.unwrap().json().await.unwrap();
            results.push(result);
        }
        host.shutdown().await.unwrap();
        std::fs::write(
            directory.join("native-host-results.json"),
            serde_json::to_vec_pretty(&results).unwrap(),
        )
        .unwrap();
        for result in results {
            assert_eq!(result["backendId"], "windows-native", "{result}");
            assert_eq!(result["status"], "ready", "{result}");
        }
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires explicitly provisioned native acceptance helpers"]
    async fn control_server_rejects_native_account_but_accepts_host_user() {
        use dsh_host_webserver::{Config, Host, WebRoute, WebRouteKind, WebServer};
        let native = std::env::var("DSH_NATIVE_ACCEPTANCE_BINARY").expect("native binary");
        let home = std::env::var("DSH_NATIVE_ACCEPTANCE_HOME").expect("native state");
        let workspace = std::env::var("DSH_NATIVE_ACCEPTANCE_WORKSPACE").expect("test workspace");
        let node = std::env::var("DSH_NATIVE_ACCEPTANCE_NODE").expect("node path");
        let ctx = cordis::Context::root();
        let server = WebServer::install_with_connection_filter(
            &ctx,
            Config {
                host: Host::Loopback,
                port: 0,
            },
            Some(filter().unwrap()),
        )
        .await
        .unwrap();
        let _route = server.register(WebRoute {
            kind: WebRouteKind::Exact,
            path: "/".into(),
            handler: Arc::new(|_| {
                Box::pin(async {
                    Ok(http::Response::new(axum::body::Body::from(
                        "HOST_CONTROL_MARKER",
                    )))
                })
            }),
        });
        let url = format!("http://127.0.0.1:{}/", server.port());
        let response = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(&url)
            .send()
            .await
            .unwrap();
        assert_eq!(response.text().await.unwrap(), "HOST_CONTROL_MARKER");
        let script = format!(
            "let r=require('http').get('{}',s=>{{s.resume();s.on('end',()=>process.exit(9))}});r.on('error',e=>{{if(['ECONNRESET','ECONNREFUSED'].includes(e.code)){{console.log('CONTROL_BLOCKED');process.exit(0)}}else{{console.error(e.code);process.exit(8)}}}});r.setTimeout(3000,()=>process.exit(7));",
            url
        );
        let output = tokio::process::Command::new(native)
            .args([
                "--native-home",
                &home,
                "--workspace",
                &workspace,
                "--mode",
                "workspace-write",
                "--command-timeout-ms",
                "5000",
                "--",
                &node,
                "-e",
                &script,
            ])
            .current_dir(workspace)
            .output()
            .await
            .unwrap();
        server.shutdown().await;
        assert!(
            output.status.success(),
            "native output: {} / {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("CONTROL_BLOCKED"));
    }
    #[tokio::test]
    async fn recognizes_a_live_same_user_tcp_peer() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = tokio::net::TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (accepted, peer) = listener.accept().await.unwrap();
        assert!(filter().unwrap()(peer, accepted.local_addr().unwrap()));
        drop(client);
    }
    #[test]
    fn unknown_connection_has_no_host_authority() {
        assert!(!filter().unwrap()(
            "127.0.0.1:1".parse().unwrap(),
            "127.0.0.1:2".parse().unwrap()
        ));
    }
}
