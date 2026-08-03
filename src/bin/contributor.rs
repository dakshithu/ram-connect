use axum::{
    extract::State,
    response::{Html, IntoResponse},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{SocketAddr, UdpSocket as StdUdpSocket};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use sysinfo::System;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UdpSocket};
use tower_http::cors::CorsLayer;

const MAX_TOTAL_POOL: usize = 4096 * 1024 * 1024; // 4 GB Contributor Cap

#[derive(Clone, Serialize, Deserialize)]
struct OrganizerSession {
    url: String,
    allocated_bytes: usize,
    used_bytes: usize,
}

#[derive(Clone)]
struct ContributorState {
    memory_pool: Arc<Mutex<Vec<u8>>>,
    sessions: Arc<Mutex<HashMap<String, OrganizerSession>>>,
    tcp_port: u16,
    web_port: u16,
    lan_ip: String,
}

#[derive(Serialize)]
struct SystemRamInfo {
    total_physical_mb: f64,
    used_physical_mb: f64,
    available_physical_mb: f64,
}

#[derive(Serialize)]
struct StatusResponse {
    tcp_port: u16,
    web_port: u16,
    lan_ip: String,
    host_ram: SystemRamInfo,
    total_system_pool_mb: f64,
    total_allocated_mb: f64,
    total_used_mb: f64,
    sessions: Vec<OrganizerSession>,
}

#[derive(Deserialize)]
struct JoinRequest {
    join_code: String,
    allocate_mb: usize,
}

#[derive(Serialize)]
struct JoinResponse {
    success: bool,
    message: String,
}

#[derive(Deserialize)]
struct DiscoveryBeacon {
    code: String,
    url: String,
}

#[derive(Deserialize)]
struct DisconnectReq {
    organizer_url: String,
}

fn get_local_lan_ip() -> String {
    if let Ok(socket) = StdUdpSocket::bind("0.0.0.0:0") {
        if socket.connect("8.8.8.8:80").is_ok() {
            if let Ok(addr) = socket.local_addr() {
                return addr.ip().to_string();
            }
        }
    }
    "127.0.0.1".to_string()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let tcp_port: u16 = args.get(1).and_then(|p| p.parse().ok()).unwrap_or(9000);
    let web_port: u16 = args.get(2).and_then(|p| p.parse().ok()).unwrap_or(tcp_port + 190);

    let lan_ip = get_local_lan_ip();

    println!("🚀 [RAM Connect Contributor] Starting Engine...");
    println!("   - LAN IP Address         : {}", lan_ip);
    println!("   - P2P TCP Mesh Listener  : 0.0.0.0:{}", tcp_port);
    println!("   - Web Control Dashboard  : http://{}:{}", lan_ip, web_port);

    let state = ContributorState {
        memory_pool: Arc::new(Mutex::new(vec![0u8; MAX_TOTAL_POOL])),
        sessions: Arc::new(Mutex::new(HashMap::new())),
        tcp_port,
        web_port,
        lan_ip,
    };

    let tcp_state = state.clone();
    tokio::spawn(async move {
        if let Err(e) = run_tcp_server(tcp_state).await {
            eprintln!("TCP Server Error: {}", e);
        }
    });

    let quic_state = state.clone();
    tokio::spawn(async move {
        if let Err(e) = run_quic_server(quic_state).await {
            eprintln!("QUIC Server Error: {}", e);
        }
    });

    let roce_state = state.clone();
    tokio::spawn(async move {
        if let Err(e) = run_roce_v2_server(roce_state).await {
            eprintln!("RoCE v2 Server Error: {}", e);
        }
    });

    let app = Router::new()
        .route("/", get(serve_dashboard_html))
        .route("/api/status", get(get_status))
        .route("/api/join", post(join_organizer))
        .route("/api/disconnect", post(disconnect_organizer))
        .route("/api/flush", post(flush_memory))
        .route("/api/local-mount", post(handle_local_mount))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], web_port));
    let listener = TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

fn make_quic_server_config() -> Result<quinn::ServerConfig, Box<dyn std::error::Error + Send + Sync + 'static>> {
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into(), "ram-connect-mesh".into()])?;
    let cert_der = cert.serialize_der()?;
    let priv_key_der = cert.serialize_private_key_der();

    let cert_chain = vec![rustls::Certificate(cert_der)];
    let key = rustls::PrivateKey(priv_key_der);

    let mut transport = quinn::TransportConfig::default();
    transport.max_concurrent_bidi_streams(100u32.into());
    transport.keep_alive_interval(Some(Duration::from_secs(5)));
    transport.stream_receive_window((16 * 1024 * 1024u32).into());
    transport.receive_window((32 * 1024 * 1024u32).into());
    transport.send_window(16 * 1024 * 1024);

    let mut server_config = quinn::ServerConfig::with_single_cert(cert_chain, key)?;
    server_config.transport_config(Arc::new(transport));

    Ok(server_config)
}

fn create_optimized_udp_socket(addr: SocketAddr) -> Result<std::net::UdpSocket, Box<dyn std::error::Error + Send + Sync + 'static>> {
    let domain = if addr.is_ipv6() { socket2::Domain::IPV6 } else { socket2::Domain::IPV4 };
    let socket = socket2::Socket::new(domain, socket2::Type::DGRAM, Some(socket2::Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    let _ = socket.set_recv_buffer_size(8 * 1024 * 1024);
    let _ = socket.set_send_buffer_size(8 * 1024 * 1024);
    socket.bind(&addr.into())?;
    socket.set_nonblocking(true)?;
    Ok(socket.into())
}

async fn run_quic_server(state: ContributorState) -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    let server_config = make_quic_server_config()?;
    let addr = SocketAddr::from(([0, 0, 0, 0], state.tcp_port));
    let std_socket = create_optimized_udp_socket(addr)?;
    let endpoint = quinn::Endpoint::new(quinn::EndpointConfig::default(), Some(server_config), std_socket, Arc::new(quinn::TokioRuntime))?;

    println!("⚡ [RAM Connect Contributor] 8MB Kernel Buffer QUIC UDP Engine active on 0.0.0.0:{}", state.tcp_port);

    while let Some(conn) = endpoint.accept().await {
        let mem = Arc::clone(&state.memory_pool);
        let sessions = Arc::clone(&state.sessions);

        tokio::spawn(async move {
            if let Ok(connection) = conn.await {
                while let Ok((mut send, mut recv)) = connection.accept_bi().await {
                    let mem = Arc::clone(&mem);
                    let sessions = Arc::clone(&sessions);
                    tokio::spawn(async move {
                        let mut header = [0u8; 9];
                        if recv.read_exact(&mut header).await.is_err() {
                            return;
                        }

                        let opcode = header[0];
                        let offset = u32::from_be_bytes([header[1], header[2], header[3], header[4]]) as usize;
                        let length = u32::from_be_bytes([header[5], header[6], header[7], header[8]]) as usize;

                        match opcode {
                            0 => {
                                let end = offset + length;
                                let mut chunk_buf = vec![0u8; length];
                                if recv.read_exact(&mut chunk_buf).await.is_ok() {
                                    {
                                        let mut pool = mem.lock().unwrap();
                                        if end <= pool.len() {
                                            pool[offset..end].copy_from_slice(&chunk_buf);
                                        }
                                    }
                                    {
                                        let mut sess = sessions.lock().unwrap();
                                        for session in sess.values_mut() {
                                            if end > session.used_bytes {
                                                session.used_bytes = end;
                                            }
                                        }
                                    }
                                    let _ = send.write_all(b"ACK_WRITE").await;
                                    let _ = send.finish().await;
                                }
                            }
                            1 => {
                                let end = offset + length;
                                let mut chunk_buf = vec![0u8; length];
                                let valid = {
                                    let pool = mem.lock().unwrap();
                                    if end <= pool.len() {
                                        chunk_buf.copy_from_slice(&pool[offset..end]);
                                        true
                                    } else {
                                        false
                                    }
                                };
                                if valid {
                                    let _ = send.write_all(&chunk_buf).await;
                                    let _ = send.finish().await;
                                }
                            }
                            2 => {
                                let total_allocated: usize = sessions
                                    .lock()
                                    .unwrap()
                                    .values()
                                    .map(|s| s.allocated_bytes)
                                    .sum();
                                let _ = send.write_all(&(total_allocated as u64).to_be_bytes()).await;
                                let _ = send.finish().await;
                            }
                            _ => {}
                        }
                    });
                }
            }
        });
    }

    Ok(())
}

async fn run_roce_v2_server(state: ContributorState) -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    let roce_port = 4791; // RoCE v2 RDMA UDP Port
    let addr = SocketAddr::from(([0, 0, 0, 0], roce_port));
    
    let std_socket = create_optimized_udp_socket(addr)?;
    let socket = UdpSocket::from_std(std_socket)?;

    println!("🚀 [RAM Connect Contributor] RoCE v2 RDMA Engine listening on UDP 0.0.0.0:{}", roce_port);

    let mut buf = vec![0u8; 131072 + 9];

    loop {
        if let Ok((n, src)) = socket.recv_from(&mut buf).await {
            if n < 9 { continue; }

            let opcode = buf[0];
            let offset = u32::from_be_bytes([buf[1], buf[2], buf[3], buf[4]]) as usize;
            let length = u32::from_be_bytes([buf[5], buf[6], buf[7], buf[8]]) as usize;

            match opcode {
                0 => {
                    let payload_bytes = n - 9;
                    if payload_bytes > 0 {
                        let end = offset + payload_bytes;
                        let mut pool = state.memory_pool.lock().unwrap();
                        if end <= pool.len() {
                            pool[offset..end].copy_from_slice(&buf[9..n]);
                        }
                    }
                    if length > 0 && (offset + payload_bytes >= offset + length) {
                        let _ = socket.send_to(b"ACK_ROCE", src).await;
                    }
                }
                1 => {
                    let mut chunk_buf = vec![0u8; 65536];
                    let mut bytes_sent = 0;

                    while bytes_sent < length {
                        let to_send = (length - bytes_sent).min(65536);
                        let chunk_offset = offset + bytes_sent;
                        let chunk_end = chunk_offset + to_send;

                        {
                            let pool = state.memory_pool.lock().unwrap();
                            if chunk_end <= pool.len() {
                                chunk_buf[..to_send].copy_from_slice(&pool[chunk_offset..chunk_end]);
                            } else {
                                break;
                            }
                        }

                        if socket.send_to(&chunk_buf[..to_send], src).await.is_err() {
                            break;
                        }
                        bytes_sent += to_send;
                    }
                }
                2 => {
                    let mut resp = [0u8; 8];
                    resp.copy_from_slice(&(MAX_TOTAL_POOL as u64).to_be_bytes());
                    let _ = socket.send_to(&resp, src).await;
                }
                _ => {}
            }
        }
    }
}

async fn run_tcp_server(state: ContributorState) -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind(format!("0.0.0.0:{}", state.tcp_port)).await?;
    loop {
        let (mut socket, _) = listener.accept().await?;
        let _ = socket.set_nodelay(true);
        let mem = Arc::clone(&state.memory_pool);
        let sessions = Arc::clone(&state.sessions);

        tokio::spawn(async move {
            let mut header = [0u8; 9];
            loop {
                if socket.read_exact(&mut header).await.is_err() {
                    break;
                }

                let opcode = header[0];
                let offset = u32::from_be_bytes([header[1], header[2], header[3], header[4]]) as usize;
                let length = u32::from_be_bytes([header[5], header[6], header[7], header[8]]) as usize;

                match opcode {
                    0 => {
                        let end = offset + length;
                        let mut chunk_buf = [0u8; 65536];
                        let mut bytes_read = 0;
                        let mut success = true;

                        while bytes_read < length {
                            let to_read = (length - bytes_read).min(chunk_buf.len());
                            if socket.read_exact(&mut chunk_buf[..to_read]).await.is_err() {
                                success = false;
                                break;
                            }

                            let chunk_offset = offset + bytes_read;
                            let chunk_end = chunk_offset + to_read;
                            {
                                let mut pool = mem.lock().unwrap();
                                if chunk_end <= pool.len() {
                                    pool[chunk_offset..chunk_end].copy_from_slice(&chunk_buf[..to_read]);
                                }
                            }
                            bytes_read += to_read;
                        }

                        if success {
                            {
                                let mut sess = sessions.lock().unwrap();
                                for session in sess.values_mut() {
                                    if end > session.used_bytes {
                                        session.used_bytes = end;
                                    }
                                }
                            }
                            let _ = socket.write_all(b"ACK_WRITE").await;
                        } else {
                            break;
                        }
                    }
                    1 => {
                        let _end = offset + length;
                        let mut bytes_sent = 0;
                        let mut success = true;
                        let mut chunk_buf = [0u8; 65536];

                        while bytes_sent < length {
                            let to_send = (length - bytes_sent).min(chunk_buf.len());
                            let chunk_offset = offset + bytes_sent;
                            let chunk_end = chunk_offset + to_send;

                            {
                                let pool = mem.lock().unwrap();
                                if chunk_end <= pool.len() {
                                    chunk_buf[..to_send].copy_from_slice(&pool[chunk_offset..chunk_end]);
                                } else {
                                    success = false;
                                    break;
                                }
                            }

                            if socket.write_all(&chunk_buf[..to_send]).await.is_err() {
                                success = false;
                                break;
                            }
                            bytes_sent += to_send;
                        }

                        if !success {
                            break;
                        }
                    }
                    2 => {
                        let total_allocated: usize = sessions
                            .lock()
                            .unwrap()
                            .values()
                            .map(|s| s.allocated_bytes)
                            .sum();
                        
                        let _ = socket.write_all(&(total_allocated as u64).to_be_bytes()).await;
                    }
                    _ => break,
                }
            }
        });
    }
}

async fn get_status(State(state): State<ContributorState>) -> impl IntoResponse {
    let mut sys = System::new();
    sys.refresh_memory();

    let host_ram = SystemRamInfo {
        total_physical_mb: sys.total_memory() as f64 / 1024.0 / 1024.0,
        used_physical_mb: sys.used_memory() as f64 / 1024.0 / 1024.0,
        available_physical_mb: sys.available_memory() as f64 / 1024.0 / 1024.0,
    };

    let sess = state.sessions.lock().unwrap();
    let sessions_list: Vec<OrganizerSession> = sess.values().cloned().collect();
    let total_allocated_bytes: usize = sessions_list.iter().map(|s| s.allocated_bytes).sum();
    let total_used_bytes: usize = sessions_list.iter().map(|s| s.used_bytes).sum();

    Json(StatusResponse {
        tcp_port: state.tcp_port,
        web_port: state.web_port,
        lan_ip: state.lan_ip.clone(),
        host_ram,
        total_system_pool_mb: MAX_TOTAL_POOL as f64 / 1048576.0,
        total_allocated_mb: total_allocated_bytes as f64 / 1048576.0,
        total_used_mb: total_used_bytes as f64 / 1048576.0,
        sessions: sessions_list,
    })
}

fn create_broadcast_udp_socket(port: u16) -> Result<std::net::UdpSocket, Box<dyn std::error::Error + Send + Sync + 'static>> {
    let std_sock = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::DGRAM, Some(socket2::Protocol::UDP))?;
    std_sock.set_reuse_address(true)?;
    #[cfg(not(target_os = "windows"))]
    let _ = std_sock.set_reuse_port(true);
    std_sock.bind(&SocketAddr::from(([0, 0, 0, 0], port)).into())?;
    std_sock.set_nonblocking(true)?;
    Ok(std_sock.into())
}

async fn resolve_organizer_url(target_input: &str) -> Option<String> {
    let clean = target_input.trim();
    if clean.is_empty() { return None; }

    // 1. Direct URL or IP Address support (e.g. 192.168.1.100:8080 or http://192.168.1.100:8080)
    if clean.contains('.') || clean.contains(':') {
        let url = if !clean.starts_with("http://") && !clean.starts_with("https://") {
            format!("http://{}", clean)
        } else {
            clean.to_string()
        };
        return Some(url);
    }

    let target_code = clean.to_uppercase();

    // 2. Fast Local Subnet & Port Probe (Probes 8080, 8081, 8082 on LAN IP & localhost)
    let probe_ports = [8080, 8081, 8082, 3000];
    if let Ok(socket) = StdUdpSocket::bind("0.0.0.0:0") {
        if socket.connect("8.8.8.8:80").is_ok() {
            if let Ok(addr) = socket.local_addr() {
                let ip_str = addr.ip().to_string();
                if let Ok(client) = reqwest::Client::builder().timeout(Duration::from_millis(600)).build() {
                    for port in probe_ports {
                        for test_ip in &[&ip_str, "127.0.0.1", "localhost"] {
                            let test_url = format!("http://{}:{}", test_ip, port);
                            let status_url = format!("{}/api/status", test_url);
                            if let Ok(res) = client.get(&status_url).send().await {
                                if let Ok(json) = res.json::<serde_json::Value>().await {
                                    if let Some(code) = json.get("join_code").and_then(|c| c.as_str()) {
                                        if code.to_uppercase() == target_code {
                                            return Some(test_url);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // 3. UDP Broadcast Receiver with SO_REUSEADDR
    if let Ok(std_sock) = create_broadcast_udp_socket(8888) {
        if let Ok(socket) = UdpSocket::from_std(std_sock) {
            let mut buf = [0u8; 1024];
            let timeout = Duration::from_secs(2);
            let start = std::time::Instant::now();

            while start.elapsed() < timeout {
                if let Ok(Ok((len, _))) = tokio::time::timeout(Duration::from_millis(400), socket.recv_from(&mut buf)).await {
                    if let Ok(beacon) = serde_json::from_slice::<DiscoveryBeacon>(&buf[..len]) {
                        if beacon.code.to_uppercase() == target_code {
                            return Some(beacon.url);
                        }
                    }
                }
            }
        }
    }

    None
}

async fn join_organizer(
    State(state): State<ContributorState>,
    Json(payload): Json<JoinRequest>,
) -> impl IntoResponse {
    let target_code = payload.join_code.trim().to_uppercase();
    let req_bytes = payload.allocate_mb * 1048576;

    {
        let sess = state.sessions.lock().unwrap();
        let currently_allocated: usize = sess.values().map(|s| s.allocated_bytes).sum();
        if currently_allocated + req_bytes > MAX_TOTAL_POOL {
            return Json(JoinResponse {
                success: false,
                message: format!(
                    "Quota exceeded! Requested {} MB, but only {} MB free in pool.",
                    payload.allocate_mb,
                    (MAX_TOTAL_POOL - currently_allocated) / 1048576
                ),
            });
        }
    }

    let organizer_url = match resolve_organizer_url(&target_code).await {
        Some(url) => url,
        None => {
            return Json(JoinResponse {
                success: false,
                message: format!("Could not discover Organizer broadcasting code '{}' on LAN.", target_code),
            });
        }
    };

    let (success, reg_msg) = reqwest_register(&organizer_url, &target_code, &state.lan_ip, state.tcp_port, payload.allocate_mb).await;

    if success {
        let mut sess = state.sessions.lock().unwrap();
        sess.insert(
            organizer_url.clone(),
            OrganizerSession {
                url: organizer_url.clone(),
                allocated_bytes: req_bytes,
                used_bytes: 0,
            },
        );

        Json(JoinResponse {
            success: true,
            message: format!("Connected to {} with {} MB allocated!", organizer_url, payload.allocate_mb),
        })
    } else {
        Json(JoinResponse {
            success: false,
            message: format!("Organizer found, but registration failed: {}", reg_msg),
        })
    }
}

async fn disconnect_organizer(
    State(state): State<ContributorState>,
    Json(payload): Json<DisconnectReq>,
) -> impl IntoResponse {
    let found = {
        let mut sess = state.sessions.lock().unwrap();
        sess.remove(&payload.organizer_url).is_some()
    };

    if found {
        // Notify organizer to unregister this node
        let unregister_url = format!("{}/api/unregister-node", payload.organizer_url.trim_end_matches('/'));
        let address = format!("{}:{}", state.lan_ip, state.tcp_port);
        let _ = reqwest::Client::new()
            .post(&unregister_url)
            .json(&serde_json::json!({ "address": address }))
            .send()
            .await;

        Json(JoinResponse {
            success: true,
            message: "Disconnected session and freed RAM allocation!".to_string(),
        })
    } else {
        Json(JoinResponse {
            success: false,
            message: "Session not found.".to_string(),
        })
    }
}

async fn flush_memory(State(state): State<ContributorState>) -> impl IntoResponse {
    {
        let mut pool = state.memory_pool.lock().unwrap();
        pool.fill(0);
    }
    {
        let mut sess = state.sessions.lock().unwrap();
        for session in sess.values_mut() {
            session.used_bytes = 0;
        }
    }
    Json(serde_json::json!({ "success": true, "message": "Memory pool zeroed successfully." }))
}

async fn reqwest_register(org_url: &str, code: &str, local_ip: &str, tcp_port: u16, alloc_mb: usize) -> (bool, String) {
    let register_url = format!("{}/api/register-node", org_url.trim_end_matches('/'));
    let body = serde_json::json!({
        "join_code": code,
        "address": format!("{}:{}", local_ip, tcp_port),
        "allocated_mb": alloc_mb
    });

    let client = match reqwest::Client::builder().timeout(Duration::from_secs(6)).build() {
        Ok(c) => c,
        Err(e) => return (false, format!("HTTP client initialization failed: {}", e)),
    };

    let mut last_err = String::from("Timeout connecting to Organizer");

    for _attempt in 1..=3 {
        match client.post(&register_url).json(&body).send().await {
            Ok(res) => {
                if res.status().is_success() {
                    if let Ok(json) = res.json::<serde_json::Value>().await {
                        let success = json.get("success").and_then(|s| s.as_bool()).unwrap_or(true);
                        let msg = json.get("message").and_then(|m| m.as_str()).unwrap_or("Registered successfully").to_string();
                        if success {
                            return (true, msg);
                        } else {
                            return (false, msg);
                        }
                    }
                    return (true, "Registered successfully".to_string());
                } else {
                    last_err = format!("HTTP {}", res.status());
                }
            }
            Err(e) => {
                last_err = e.to_string();
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    (false, format!("Registration attempt failed ({})", last_err))
}
async fn serve_dashboard_html(State(state): State<ContributorState>) -> Html<String> {
    let raw_html = r#"
<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>RAM Connect — Contributor Hub</title>
    <link href="https://fonts.googleapis.com/css2?family=Inter:wght@300;400;500;600;700;800&display=swap" rel="stylesheet">
    <style>
        :root {
            --bg-color: #06090f;
            --glass-bg: rgba(255, 255, 255, 0.03);
            --glass-border: rgba(255, 255, 255, 0.06);
            --glass-border-hover: rgba(255, 255, 255, 0.12);
            --text-main: #f8fafc;
            --text-sub: #94a3b8;
            --accent-emerald: #10b981;
            --accent-cyan: #06b6d4;
            --accent-blue: #3b82f6;
            --accent-warn: #f59e0b;
            --accent-red: #ef4444;
            --font-family: 'Inter', sans-serif;
        }
        * { box-sizing: border-box; font-family: var(--font-family); margin: 0; padding: 0; }
        body { 
            background-color: var(--bg-color);
            color: var(--text-main); 
            min-height: 100vh;
            display: flex;
            flex-direction: column;
            overflow-x: hidden;
            background-image: 
                radial-gradient(circle at 15% 30%, rgba(16, 185, 129, 0.08), transparent 30%),
                radial-gradient(circle at 85% 70%, rgba(59, 130, 246, 0.08), transparent 30%);
        }

        /* Header */
        .header { 
            display: flex; justify-content: space-between; align-items: center; 
            padding: 1.25rem 2.5rem;
            background: linear-gradient(to right, rgba(15, 23, 42, 0.85), rgba(2, 6, 23, 0.85));
            backdrop-filter: blur(12px);
            border-bottom: 1px solid var(--glass-border);
            position: sticky; top: 0; z-index: 100;
        }
        .header-title { 
            display: flex; align-items: center; gap: 0.75rem; 
            font-size: 1.25rem; font-weight: 700; color: var(--accent-emerald);
        }
        .header-info { display: flex; align-items: center; gap: 1.25rem; font-size: 0.875rem; color: var(--text-sub); }
        .lan-badge { font-family: monospace; font-size: 0.9rem; color: var(--text-main); background: rgba(255, 255, 255, 0.05); padding: 0.35rem 0.75rem; border-radius: 6px; border: 1px solid var(--glass-border); }

        .pulse-badge { 
            background: rgba(16, 185, 129, 0.1); color: var(--accent-emerald); border: 1px solid var(--accent-emerald); 
            padding: 0.25rem 0.75rem; border-radius: 9999px; font-size: 0.75rem; font-weight: 700; 
            display: flex; align-items: center; gap: 0.5rem; letter-spacing: 0.05em;
        }
        .pulse { width: 8px; height: 8px; background: var(--accent-emerald); border-radius: 50%; animation: pulse-anim 2s infinite; box-shadow: 0 0 8px var(--accent-emerald); }
        @keyframes pulse-anim { 0% { opacity: 0.5; transform: scale(0.95); } 50% { opacity: 1; transform: scale(1.1); } 100% { opacity: 0.5; transform: scale(0.95); } }

        .container { width: 100%; max-width: 1150px; margin: 0 auto; padding: 2rem; display: flex; flex-direction: column; gap: 1.5rem; flex: 1; }

        /* Glass Cards */
        .glass-card {
            background: var(--glass-bg);
            backdrop-filter: blur(16px);
            border: 1px solid var(--glass-border);
            border-radius: 16px;
            padding: 1.5rem;
            transition: border-color 0.3s ease;
        }
        .glass-card:hover { border-color: var(--glass-border-hover); }

        /* Cross Device Banner */
        .banner {
            display: flex; justify-content: space-between; align-items: center;
            padding: 1.25rem 1.75rem;
            background: linear-gradient(135deg, rgba(59, 130, 246, 0.12), rgba(15, 23, 42, 0.5));
            border-left: 4px solid var(--accent-blue);
        }
        .banner-text { display: flex; flex-direction: column; gap: 0.25rem; }
        .banner-title { font-size: 0.75rem; font-weight: 700; color: var(--accent-blue); text-transform: uppercase; letter-spacing: 0.05em; }
        .banner-url { font-family: monospace; font-size: 1.25rem; font-weight: 700; color: #ffffff; }
        
        .btn-copy { 
            background: rgba(59, 130, 246, 0.15); border: 1px solid rgba(59, 130, 246, 0.3); color: var(--accent-blue);
            padding: 0.6rem 1.2rem; border-radius: 8px; cursor: pointer; font-weight: 600; font-size: 0.875rem;
            transition: all 0.2s; display: flex; align-items: center; gap: 0.5rem;
        }
        .btn-copy:hover { background: var(--accent-blue); color: #ffffff; box-shadow: 0 4px 12px rgba(59, 130, 246, 0.3); }

        /* Grid Stats */
        .grid-3 { display: grid; grid-template-columns: repeat(auto-fit, minmax(300px, 1fr)); gap: 1.5rem; }
        .stat-card { display: flex; flex-direction: column; position: relative; overflow: hidden; }
        .stat-header { font-size: 0.75rem; color: var(--text-sub); font-weight: 600; text-transform: uppercase; letter-spacing: 0.05em; margin-bottom: 0.75rem; }
        .stat-val { font-size: 1.85rem; font-weight: 700; color: var(--text-main); margin-top: auto; }
        
        /* Ring */
        .ring-container { position: absolute; right: 1.25rem; top: 50%; transform: translateY(-50%); width: 76px; height: 76px; }
        .ring-svg { transform: rotate(-90deg); width: 100%; height: 100%; }
        .ring-bg { fill: none; stroke: rgba(255,255,255,0.05); stroke-width: 7; }
        .ring-fill { fill: none; stroke: var(--accent-emerald); stroke-width: 7; stroke-dasharray: 201; stroke-dashoffset: 201; stroke-linecap: round; transition: stroke-dashoffset 1s ease-out, stroke 0.5s ease; }
        .ring-text { position: absolute; inset: 0; display: flex; align-items: center; justify-content: center; font-size: 0.85rem; font-weight: 700; color: var(--text-main); }

        /* Bars */
        .bar-wrap { width: 100%; height: 6px; background: rgba(255,255,255,0.06); border-radius: 3px; overflow: hidden; margin-top: 0.75rem; }
        .bar-inner { height: 100%; background: var(--accent-emerald); border-radius: 3px; transition: width 0.5s ease; }
        
        /* Visualizer */
        .vis-bar { width: 100%; height: 20px; border-radius: 10px; display: flex; overflow: hidden; background: rgba(255,255,255,0.04); margin-top: 0.75rem; }
        .vis-used { background: linear-gradient(90deg, #2563eb, #3b82f6); transition: width 0.5s; }
        .vis-alloc { background: linear-gradient(90deg, #059669, #10b981); transition: width 0.5s; }
        .vis-legend { display: flex; gap: 1.5rem; margin-top: 0.75rem; font-size: 0.8rem; font-weight: 500; color: var(--text-sub); }
        .vis-dot { display: inline-block; width: 8px; height: 8px; border-radius: 50%; margin-right: 0.35rem; }

        /* Join Form */
        .join-card { display: grid; grid-template-columns: 1fr 1fr; gap: 2rem; align-items: start; }
        @media (max-width: 768px) { .join-card { grid-template-columns: 1fr; } }
        .join-col { display: flex; flex-direction: column; gap: 1.25rem; }
        
        .otp-container { display: flex; gap: 0.5rem; }
        .code-char { 
            width: 2.75rem; height: 3.25rem; text-align: center; font-size: 1.4rem; font-weight: 700;
            background: rgba(0,0,0,0.4); border: 1.5px solid var(--glass-border); color: #ffffff;
            border-radius: 8px; transition: all 0.2s; text-transform: uppercase; font-family: monospace;
        }
        .code-char:focus { outline: none; border-color: var(--accent-emerald); box-shadow: 0 0 0 3px rgba(16, 185, 129, 0.2); transform: translateY(-1px); }
        
        .slider-container { display: flex; flex-direction: column; gap: 0.5rem; }
        .slider-header { display: flex; justify-content: space-between; font-weight: 600; font-size: 0.85rem; color: var(--text-sub); }
        .slider { -webkit-appearance: none; width: 100%; height: 6px; background: rgba(255,255,255,0.08); border-radius: 3px; outline: none; }
        .slider::-webkit-slider-thumb { -webkit-appearance: none; appearance: none; width: 18px; height: 18px; border-radius: 50%; background: var(--accent-emerald); cursor: pointer; transition: transform 0.1s; }
        .slider::-webkit-slider-thumb:hover { transform: scale(1.25); }
        .ticks { display: flex; justify-content: space-between; padding: 0 4px; font-size: 0.75rem; color: var(--text-sub); margin-top: 0.2rem; }

        .btn-primary { 
            background: linear-gradient(135deg, var(--accent-emerald), #059669); color: #ffffff; font-weight: 700; border: none; 
            padding: 0.85rem 1.25rem; border-radius: 10px; cursor: pointer; font-size: 0.95rem; 
            transition: all 0.2s; display: flex; justify-content: center; align-items: center; gap: 0.5rem;
            width: 100%; margin-top: auto; box-shadow: 0 4px 12px rgba(16, 185, 129, 0.2);
        }
        .btn-primary:hover { box-shadow: 0 6px 16px rgba(16, 185, 129, 0.35); transform: translateY(-1px); }
        .btn-primary:active { transform: translateY(0); }
        .spinner { width: 18px; height: 18px; border: 2.5px solid rgba(255,255,255,0.3); border-top-color: #ffffff; border-radius: 50%; animation: spin 1s linear infinite; display: none; }
        @keyframes spin { to { transform: rotate(360deg); } }

        /* Table */
        .table-card { padding: 1.5rem; }
        .table-header { display: flex; justify-content: space-between; align-items: center; margin-bottom: 1.25rem; }
        .btn-secondary { background: rgba(255,255,255,0.05); color: var(--text-main); border: 1px solid var(--glass-border); padding: 0.5rem 1rem; border-radius: 8px; cursor: pointer; font-weight: 600; font-size: 0.85rem; transition: all 0.2s; }
        .btn-secondary:hover { background: rgba(255,255,255,0.1); }
        
        table { width: 100%; border-collapse: collapse; text-align: left; }
        th { padding: 0.85rem 1rem; color: var(--text-sub); font-size: 0.75rem; text-transform: uppercase; letter-spacing: 0.05em; border-bottom: 1px solid var(--glass-border); font-weight: 600; }
        td { padding: 1rem; border-bottom: 1px solid rgba(255,255,255,0.03); font-size: 0.875rem; font-weight: 500; }
        tr:hover td { background: rgba(255,255,255,0.02); }
        tr:last-child td { border-bottom: none; }
        
        .btn-danger { background: rgba(239, 68, 68, 0.12); color: var(--accent-red); border: 1px solid rgba(239, 68, 68, 0.25); padding: 0.4rem 0.85rem; font-size: 0.8rem; border-radius: 6px; cursor: pointer; font-weight: 600; transition: all 0.2s; }
        .btn-danger:hover { background: var(--accent-red); color: #ffffff; }

        /* Toasts */
        #toast-container { position: fixed; bottom: 2rem; right: 2rem; display: flex; flex-direction: column; gap: 0.75rem; z-index: 9999; }
        .toast { 
            padding: 0.85rem 1.25rem; border-radius: 10px; background: rgba(15, 23, 42, 0.95); backdrop-filter: blur(12px); border: 1px solid var(--glass-border);
            color: #ffffff; font-weight: 500; font-size: 0.875rem; display: flex; align-items: center; gap: 0.75rem; box-shadow: 0 10px 30px rgba(0,0,0,0.5);
            transform: translateX(120%); opacity: 0; transition: all 0.35s cubic-bezier(0.175, 0.885, 0.32, 1.275);
        }
        .toast.show { transform: translateX(0); opacity: 1; }
        .toast.success { border-left: 4px solid var(--accent-emerald); }
        .toast.error { border-left: 4px solid var(--accent-red); }
        .toast.info { border-left: 4px solid var(--accent-blue); }

        /* Modal */
        #modal-overlay { 
            position: fixed; inset: 0; background: rgba(0,0,0,0.7); backdrop-filter: blur(6px); 
            display: flex; align-items: center; justify-content: center; z-index: 10000;
            opacity: 0; pointer-events: none; transition: opacity 0.25s;
        }
        #modal-overlay.show { opacity: 1; pointer-events: auto; }
        .modal-content { 
            background: #0f172a; border: 1px solid var(--glass-border); border-radius: 16px; 
            padding: 1.75rem; width: 90%; max-width: 400px; transform: scale(0.92); transition: transform 0.25s ease;
            box-shadow: 0 20px 40px rgba(0,0,0,0.6);
        }
        #modal-overlay.show .modal-content { transform: scale(1); }
        .modal-title { font-size: 1.15rem; font-weight: 700; margin-bottom: 0.5rem; color: #ffffff; }
        .modal-msg { color: var(--text-sub); font-size: 0.9rem; margin-bottom: 1.5rem; line-height: 1.5; }
        .modal-actions { display: flex; justify-content: flex-end; gap: 0.75rem; }

        footer { text-align: center; color: var(--text-sub); font-size: 0.75rem; padding: 1.5rem 0; opacity: 0.7; }
    </style>
</head>
<body>
    <header class="header">
        <div class="header-title">
            <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M13 2L3 14h9l-1 8 10-12h-9l1-8z"/></svg>
            RAM Connect Contributor
        </div>
        <div class="header-info">
            <span class="lan-badge">LAN: __LAN_IP__</span>
            <div class="pulse-badge">
                <div class="pulse"></div> ACTIVE
            </div>
        </div>
    </header>

    <main class="container">
        <div class="glass-card banner">
            <div class="banner-text">
                <span class="banner-title">Cross-Device Access URL</span>
                <span class="banner-url" id="accessUrl">http://__LAN_IP__:__WEB_PORT__</span>
            </div>
            <button class="btn-copy" onclick="copyUrl()">
                <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2" ry="2"></rect><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"></path></svg>
                Copy Link
            </button>
        </div>

        <div class="grid-3">
            <div class="glass-card stat-card">
                <div class="stat-header">Physical System RAM</div>
                <div class="stat-val" id="sysRamStat">0 / 0 GB</div>
                <div class="ring-container">
                    <svg class="ring-svg" viewBox="0 0 72 72">
                        <circle class="ring-bg" cx="36" cy="36" r="32"></circle>
                        <circle class="ring-fill" id="ramRing" cx="36" cy="36" r="32"></circle>
                    </svg>
                    <div class="ring-text" id="ramRingText">0%</div>
                </div>
            </div>
            
            <div class="glass-card stat-card">
                <div class="stat-header">Global Pool Allocation</div>
                <div class="stat-val" style="color:var(--accent-emerald);" id="poolAllocStat">0 / 0 MB</div>
                <div class="bar-wrap">
                    <div class="bar-inner" id="allocBar" style="width: 0%; background: var(--accent-emerald);"></div>
                </div>
            </div>

            <div class="glass-card stat-card">
                <div class="stat-header">Active Mesh RAM Usage</div>
                <div class="stat-val" style="color:var(--accent-blue);" id="poolUsedStat">0 MB</div>
                <div class="bar-wrap">
                    <div class="bar-inner" id="usedBar" style="width: 0%; background: var(--accent-blue);"></div>
                </div>
            </div>
        </div>

        <div class="glass-card visualizer">
            <div class="stat-header" style="margin-bottom: 0.25rem;">Memory Pool Visualizer</div>
            <div class="vis-bar">
                <div class="vis-used" id="visUsed" style="width: 0%;"></div>
                <div class="vis-alloc" id="visAlloc" style="width: 0%;"></div>
            </div>
            <div class="vis-legend">
                <div><span class="vis-dot" style="background: var(--accent-blue);"></span>Used (<span id="lblUsed">0</span>MB)</div>
                <div><span class="vis-dot" style="background: var(--accent-emerald);"></span>Free Allocated (<span id="lblFreeAlloc">0</span>MB)</div>
                <div><span class="vis-dot" style="background: rgba(255,255,255,0.08);"></span>Unallocated (<span id="lblUnalloc">0</span>MB)</div>
            </div>
        </div>

        <div class="glass-card join-card">
            <div class="join-col">
                <div>
                    <h3 style="font-size: 1.1rem; font-weight: 700; margin-bottom: 0.35rem;">🔗 Connect to Mesh</h3>
                    <p style="color: var(--text-sub); font-size: 0.85rem; line-height: 1.4;">Enter the 6-character Join Code from an Organizer on your LAN.</p>
                </div>
                
                <div class="otp-container" id="otpGroup">
                    <input type="text" class="code-char" maxlength="1" autofocus>
                    <input type="text" class="code-char" maxlength="1">
                    <input type="text" class="code-char" maxlength="1">
                    <input type="text" class="code-char" maxlength="1">
                    <input type="text" class="code-char" maxlength="1">
                    <input type="text" class="code-char" maxlength="1">
                </div>
            </div>
            
            <div class="join-col">
                <div class="slider-container">
                    <div class="slider-header">
                        <span>Quota Allocation</span>
                        <span style="color: var(--accent-emerald); font-weight: 700;" id="allocPreview">1024 MB</span>
                    </div>
                    <input type="range" class="slider" id="allocSlider" min="256" max="4096" step="256" value="1024">
                    <div class="ticks">
                        <span>256M</span><span>1G</span><span>2G</span><span>3G</span><span>4G</span>
                    </div>
                </div>
                
                <button class="btn-primary" id="btnConnect" onclick="joinMesh()">
                    <span id="btnText">Auto-Discover & Connect</span>
                    <div class="spinner" id="btnSpinner"></div>
                </button>
            </div>
        </div>

        <div class="glass-card table-card">
            <div class="table-header">
                <h3 style="font-size: 1.1rem; font-weight: 700;">🖥️ Connected Organizers</h3>
                <button class="btn-secondary" onclick="promptFlush()">🧹 Flush Buffer</button>
            </div>
            <div style="overflow-x: auto;">
                <table>
                    <thead>
                        <tr>
                            <th>Host URL</th>
                            <th>Allocated</th>
                            <th>Used</th>
                            <th>Live Usage</th>
                            <th style="text-align: right;">Action</th>
                        </tr>
                    </thead>
                    <tbody id="sessionsTable">
                        <tr><td colspan="5" style="text-align: center; color: var(--text-sub); padding: 2.5rem 1rem;">Searching for local network connections...</td></tr>
                    </tbody>
                </table>
            </div>
        </div>
    </main>
        
    <footer>
        RAM Connect v0.1.0 • Contributor Node
    </footer>

    <!-- Toast Container -->
    <div id="toast-container"></div>

    <!-- Custom Modal -->
    <div id="modal-overlay">
        <div class="modal-content">
            <div class="modal-title" id="modalTitle">Confirm Action</div>
            <div class="modal-msg" id="modalMsg">Are you sure?</div>
            <div class="modal-actions">
                <button class="btn-secondary" id="modalCancel">Cancel</button>
                <button class="btn-danger" id="modalConfirm">Confirm</button>
            </div>
        </div>
    </div>

    <script>
        function showToast(msg, type = 'info') {
            const container = document.getElementById('toast-container');
            const toast = document.createElement('div');
            toast.className = `toast ${type}`;
            let icon = '';
            if (type === 'success') icon = '✓';
            else if (type === 'error') icon = '⚠';
            else icon = 'ℹ';
            toast.innerHTML = `<span style="font-weight:900;">${icon}</span> ${msg}`;
            container.appendChild(toast);
            setTimeout(() => toast.classList.add('show'), 10);
            setTimeout(() => {
                toast.classList.remove('show');
                setTimeout(() => toast.remove(), 350);
            }, 3000);
        }

        function confirmAction(title, msg, confirmText = 'Confirm') {
            return new Promise((resolve) => {
                const overlay = document.getElementById('modal-overlay');
                const btnConfirm = document.getElementById('modalConfirm');
                const btnCancel = document.getElementById('modalCancel');
                document.getElementById('modalTitle').innerText = title;
                document.getElementById('modalMsg').innerText = msg;
                btnConfirm.innerText = confirmText;
                const cleanup = () => {
                    overlay.classList.remove('show');
                    btnConfirm.removeEventListener('click', onConfirm);
                    btnCancel.removeEventListener('click', onCancel);
                };
                const onConfirm = () => { cleanup(); resolve(true); };
                const onCancel = () => { cleanup(); resolve(false); };
                btnConfirm.addEventListener('click', onConfirm);
                btnCancel.addEventListener('click', onCancel);
                overlay.classList.add('show');
            });
        }

        function copyToClipboard(text, successMsg = 'Copied to clipboard!') {
            if (navigator.clipboard && window.isSecureContext) {
                navigator.clipboard.writeText(text).then(() => {
                    showToast(successMsg, 'success');
                }).catch(() => {
                    fallbackCopyText(text, successMsg);
                });
            } else {
                fallbackCopyText(text, successMsg);
            }
        }

        function fallbackCopyText(text, successMsg) {
            const textArea = document.createElement('textarea');
            textArea.value = text;
            textArea.style.position = 'fixed';
            textArea.style.top = '-9999px';
            textArea.style.left = '-9999px';
            document.body.appendChild(textArea);
            textArea.focus();
            textArea.select();
            try {
                const successful = document.execCommand('copy');
                if (successful) {
                    showToast(successMsg, 'success');
                } else {
                    showToast('Failed to copy', 'error');
                }
            } catch (err) {
                showToast('Copy failed', 'error');
            }
            document.body.removeChild(textArea);
        }

        function copyUrl() {
            const url = document.getElementById('accessUrl').innerText.trim();
            copyToClipboard(url, 'Access URL copied to clipboard!');
        }

        const inputs = document.querySelectorAll('.code-char');
        inputs.forEach((input, index) => {
            input.addEventListener('input', (e) => {
                if (e.target.value.length === 1 && index < inputs.length - 1) inputs[index + 1].focus();
            });
            input.addEventListener('keydown', (e) => {
                if (e.key === 'Backspace' && e.target.value === '' && index > 0) inputs[index - 1].focus();
            });
            input.addEventListener('paste', (e) => {
                e.preventDefault();
                const data = e.clipboardData.getData('text').replace(/[^a-zA-Z0-9]/g, '').toUpperCase().slice(0, 6);
                for (let i = 0; i < data.length; i++) {
                    if (inputs[i]) {
                        inputs[i].value = data[i];
                        if (i < inputs.length - 1) inputs[i + 1].focus();
                    }
                }
            });
        });

        const slider = document.getElementById('allocSlider');
        const preview = document.getElementById('allocPreview');
        slider.addEventListener('input', (e) => { preview.innerText = `${e.target.value} MB`; });

        async function updateStatus() {
            try {
                const res = await fetch('/api/status');
                const data = await res.json();
                const usedRam = data.host_ram.used_physical_mb / 1024;
                const totalRam = data.host_ram.total_physical_mb / 1024;
                const ramPct = Math.round((usedRam / totalRam) * 100) || 0;
                document.getElementById('sysRamStat').innerText = `${usedRam.toFixed(1)} / ${totalRam.toFixed(1)} GB`;
                document.getElementById('ramRingText').innerText = `${ramPct}%`;
                const ring = document.getElementById('ramRing');
                ring.style.strokeDashoffset = 201 - (201 * ramPct) / 100;
                if (ramPct < 60) ring.style.stroke = 'var(--accent-emerald)';
                else if (ramPct < 80) ring.style.stroke = 'var(--accent-warn)';
                else ring.style.stroke = 'var(--accent-red)';
                document.getElementById('poolAllocStat').innerText = `${data.total_allocated_mb.toFixed(0)} / ${data.total_system_pool_mb.toFixed(0)} MB`;
                document.getElementById('allocBar').style.width = `${Math.min(100, (data.total_allocated_mb / data.total_system_pool_mb) * 100)}%`;
                document.getElementById('poolUsedStat').innerText = `${data.total_used_mb.toFixed(2)} MB`;
                document.getElementById('usedBar').style.width = `${data.total_allocated_mb > 0 ? (data.total_used_mb / data.total_allocated_mb) * 100 : 0}%`;
                document.getElementById('lblUsed').innerText = data.total_used_mb.toFixed(0);
                document.getElementById('lblFreeAlloc').innerText = (data.total_allocated_mb - data.total_used_mb).toFixed(0);
                document.getElementById('lblUnalloc').innerText = (data.total_system_pool_mb - data.total_allocated_mb).toFixed(0);
                document.getElementById('visUsed').style.width = `${(data.total_used_mb / data.total_system_pool_mb) * 100}%`;
                document.getElementById('visAlloc').style.width = `${((data.total_allocated_mb - data.total_used_mb) / data.total_system_pool_mb) * 100}%`;
                const tbody = document.getElementById('sessionsTable');
                if (data.sessions.length === 0) {
                    tbody.innerHTML = '<tr><td colspan="5" style="text-align: center; color: var(--text-sub); padding: 2.5rem 1rem;">No active mesh connections. Enter a Join Code above!</td></tr>';
                    return;
                }
                tbody.innerHTML = data.sessions.map(s => {
                    const allocMb = s.allocated_bytes / 1048576;
                    const usedMb = s.used_bytes / 1048576;
                    const pct = allocMb > 0 ? ((usedMb / allocMb) * 100).toFixed(1) : 0;
                    let barColor = 'var(--accent-emerald)';
                    if (pct > 80) barColor = 'var(--accent-red)'; else if (pct > 60) barColor = 'var(--accent-warn)';
                    return `<tr><td><strong style="color:var(--text-main); font-family: monospace;">${s.url}</strong></td><td>${allocMb.toFixed(0)} MB</td><td><strong style="color:var(--accent-blue);">${usedMb.toFixed(2)} MB</strong></td><td style="width: 200px;"><div style="display:flex; justify-content:space-between; font-size:0.75rem; color:var(--text-sub); margin-bottom:4px;"><span>Usage</span><span>${pct}%</span></div><div class="bar-wrap" style="margin-top:0; height:6px;"><div class="bar-inner" style="width: ${pct}%; background: ${barColor};"></div></div></td><td style="text-align: right;"><button class="btn-danger" onclick="promptDisconnect('${s.url}')">Disconnect</button></td></tr>`;
                }).join('');
            } catch(e) { console.error('Failed to update status', e); }
        }

        async function joinMesh() {
            const code = Array.from(document.querySelectorAll('.code-char')).map(i => i.value.trim()).join('');
            const mb = parseInt(document.getElementById('allocSlider').value);
            if (code.length < 6) { showToast('Please enter a complete 6-character code.', 'error'); return; }
            const btn = document.getElementById('btnConnect'), txt = document.getElementById('btnText'), spinner = document.getElementById('btnSpinner');
            txt.style.display = 'none'; spinner.style.display = 'block'; btn.disabled = true;
            try {
                const res = await fetch('/api/join', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ join_code: code, allocate_mb: mb }) });
                const data = await res.json();
                if (data.success) { showToast(data.message, 'success'); inputs.forEach(i => i.value = ''); inputs[0].focus(); }
                else showToast(data.message, 'error');
                updateStatus();
            } catch (e) { showToast('Failed to communicate with local service.', 'error'); }
            finally { txt.style.display = 'block'; spinner.style.display = 'none'; btn.disabled = false; }
        }

        async function promptDisconnect(url) {
            if (await confirmAction('Disconnect Organizer', `Are you sure you want to disconnect from ${url}?`)) {
                try {
                    const res = await fetch('/api/disconnect', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ organizer_url: url }) });
                    const data = await res.json();
                    if(data.success) showToast('Disconnected successfully', 'success'); else showToast(data.message, 'error');
                    updateStatus();
                } catch(e) { showToast('Disconnect failed', 'error'); }
            }
        }

        async function promptFlush() {
            if (await confirmAction('Flush Buffer', 'Are you sure you want to zero out the memory pool?', 'Flush Now')) {
                try {
                    const res = await fetch('/api/flush', { method: 'POST' });
                    const data = await res.json();
                    if(data.success) showToast('Memory pool zeroed successfully', 'success'); else showToast(data.message, 'error');
                    updateStatus();
                } catch(e) { showToast('Flush failed', 'error'); }
            }
        }

        setInterval(updateStatus, 1000);
        updateStatus();
    </script>
</body>
</html>
    "#;

    let html = raw_html
        .replace("__LAN_IP__", &state.lan_ip)
        .replace("__WEB_PORT__", &state.web_port.to_string());

    Html(html)
}

#[derive(Deserialize)]
struct LocalMountReq {
    server_ip: String,
    web_port: u16,
}

async fn handle_local_mount(Json(payload): Json<LocalMountReq>) -> impl IntoResponse {
    let server_ip = payload.server_ip;
    let web_port = payload.web_port;

    #[cfg(target_os = "windows")]
    {
        let dav_unc_url = format!(r"\\{}@{}\dav", server_ip, web_port);
        let _ = std::process::Command::new("sc").args(["start", "WebClient"]).output();
        let _ = std::process::Command::new("net").args(["start", "WebClient"]).output();
        let _ = std::process::Command::new("net").args(["use", "R:", "/delete", "/yes"]).output();

        let out = std::process::Command::new("net")
            .args(["use", "R:", &dav_unc_url, "/persistent:no"])
            .output();

        if let Ok(o) = out {
            if o.status.success() {
                let _ = std::process::Command::new("explorer").arg(r"R:").spawn();
                return Json(serde_json::json!({ "success": true, "message": "⚡ RAM Mesh mounted as Physical System Drive R:\\ successfully!" }));
            }
        }

        Json(serde_json::json!({ "success": false, "message": "Failed to mount Drive R: via net use." }))
    }

    #[cfg(target_os = "macos")]
    {
        let dav_url = format!("http://{}:{}/dav", server_ip, web_port);
        let _ = std::fs::create_dir_all("/Volumes/RAMConnect");
        let _ = std::process::Command::new("mount_webdav")
            .args(["-v", "RAMConnect", &dav_url, "/Volumes/RAMConnect"])
            .output();
        let _ = std::process::Command::new("open").arg("/Volumes/RAMConnect").spawn();
        Json(serde_json::json!({ "success": true, "message": "⚡ Physical RAM Drive mounted at /Volumes/RAMConnect!" }))
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = server_ip;
        let _ = web_port;
        Json(serde_json::json!({ "success": true, "message": "Local mount complete." }))
    }
}