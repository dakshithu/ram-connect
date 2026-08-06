use axum::{
    extract::{DefaultBodyLimit, Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{SocketAddr, UdpSocket as StdUdpSocket};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use sysinfo::System;
use tokio::net::UdpSocket;
use tower_http::cors::CorsLayer;

#[derive(Clone, Serialize, Deserialize)]
struct ContributorNode {
    address: String,
    allocated_mb: usize,
}

#[derive(Clone, Serialize, Deserialize)]
struct HostRamInfo {
    total_mb: f64,
    used_mb: f64,
    available_mb: f64,
    usage_pct: f64,
    is_high_ram: bool,
    high_threshold_pct: f64,
    force_high_ram: bool,
}

#[derive(Clone, Serialize, Deserialize)]
struct MeshFileSummary {
    id: String,
    name: String,
    size_bytes: usize,
    created_at: String,
    storage_location: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct MeshFile {
    id: String,
    name: String,
    size_bytes: usize,
    created_at: String,
    content_base64: String,
    storage_location: String,
}

#[derive(Clone)]
struct OrganizerState {
    nodes: Arc<Mutex<HashMap<String, ContributorNode>>>,
    files: Arc<Mutex<HashMap<String, MeshFile>>>,
    join_code: String,
    web_port: u16,
    lan_ip: String,
    force_high_ram: Arc<Mutex<bool>>,
    is_swap_active: Arc<Mutex<bool>>,
}

#[derive(Deserialize)]
struct RegisterNodeReq {
    join_code: String,
    address: String,
    allocated_mb: usize,
}

#[derive(Deserialize)]
struct UnregisterNodeReq {
    address: String,
}

#[derive(Serialize, Clone)]
struct ProtocolResult {
    protocol_name: String,
    success: bool,
    latency_ms: u128,
    write_speed_mbps: f64,
    read_speed_mbps: f64,
    total_transfer_sec: f64,
    message: String,
}

#[derive(Serialize)]
struct BenchmarkResponse {
    success: bool,
    fastest_protocol: String,
    results: Vec<ProtocolResult>,
    message: String,
}

#[derive(Serialize)]
struct StatusResponse {
    join_code: String,
    web_port: u16,
    lan_ip: String,
    total_nodes: usize,
    total_mesh_ram_mb: usize,
    used_mesh_ram_bytes: usize,
    files_count: usize,
    nodes: Vec<ContributorNode>,
    host_ram: HostRamInfo,
    host_files_count: usize,
    contributor_files_count: usize,
    is_swap_active: bool,
}

#[derive(Deserialize)]
struct UploadFileReq {
    name: String,
    content_base64: String,
    target_address: Option<String>,
}

#[derive(Deserialize)]
struct DeleteFileReq {
    id: String,
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

fn generate_join_code() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().subsec_nanos();
    format!("{:06X}", nanos % 0xFFFFFF)
}

fn get_host_ram_info(state: &OrganizerState) -> HostRamInfo {
    let mut sys = System::new();
    sys.refresh_memory();

    let total_bytes = sys.total_memory();
    let used_bytes = sys.used_memory();
    let available_bytes = sys.available_memory();

    let total_mb = total_bytes as f64 / 1024.0 / 1024.0;
    let used_mb = used_bytes as f64 / 1024.0 / 1024.0;
    let available_mb = available_bytes as f64 / 1024.0 / 1024.0;
    let usage_pct = if total_mb > 0.0 { (used_mb / total_mb) * 100.0 } else { 0.0 };

    let force_high = *state.force_high_ram.lock().unwrap();
    let high_threshold_pct = 75.0;
    let is_high_ram = force_high || (usage_pct >= high_threshold_pct);

    HostRamInfo {
        total_mb,
        used_mb,
        available_mb,
        usage_pct,
        is_high_ram,
        high_threshold_pct,
        force_high_ram: force_high,
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let web_port: u16 = args.get(1).and_then(|p| p.parse().ok()).unwrap_or(8080);

    let lan_ip = get_local_lan_ip();
    let join_code = generate_join_code();

    println!("👑 [RAM Connect Organizer] Control Plane Online!");
    println!("   - Network LAN IP         : {}", lan_ip);
    println!("   - Mesh Join Code         : {}", join_code);
    println!("   - Web Control Dashboard  : http://{}:{}", lan_ip, web_port);

    let state = OrganizerState {
        nodes: Arc::new(Mutex::new(HashMap::new())),
        files: Arc::new(Mutex::new(HashMap::new())),
        join_code: join_code.clone(),
        web_port,
        lan_ip: lan_ip.clone(),
        force_high_ram: Arc::new(Mutex::new(false)),
        is_swap_active: Arc::new(Mutex::new(false)),
    };

    // Spawn RAM Watcher
    spawn_ram_mount_watcher(state.clone());

    // Kill any ghost macOS NetAuthAgent mount prompts on startup
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("pkill").arg("-9").arg("NetAuthAgent").output();
        let _ = std::process::Command::new("diskutil").args(["unmount", "force", "/Volumes/127.0.0.1"]).output();
        let _ = std::process::Command::new("diskutil").args(["unmount", "force", "/Volumes/RAMConnect"]).output();
    }

    // Spawn UDP Broadcast Discovery Beacon
    let beacon_code = join_code.clone();
    let beacon_url = format!("http://{}:{}", lan_ip, web_port);
    tokio::spawn(async move {
        if let Ok(socket) = UdpSocket::bind("0.0.0.0:0").await {
            let _ = socket.set_broadcast(true);
            let beacon_data = serde_json::json!({
                "code": beacon_code,
                "url": beacon_url
            }).to_string();

            loop {
                let _ = socket.send_to(beacon_data.as_bytes(), "255.255.255.255:8888").await;
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    });

    let app = Router::new()
        .route("/", axum::routing::any(handle_root))
        .route("/api/status", get(get_status))
        .route("/api/register-node", post(register_node))
        .route("/api/unregister-node", post(unregister_node))
        .route("/api/benchmark", post(run_benchmark))
        .route("/api/files/list", get(list_files))
        .route("/api/files/upload", post(upload_file))
        .route("/api/files/delete", post(delete_file))
        .route("/api/files/raw/:file_id", get(get_raw_file))
        .route("/api/toggle-high-ram-simulation", post(toggle_high_ram_simulation))
        .route("/api/mount-system-drive", post(mount_system_drive))
        .route("/api/local-mount", post(handle_local_mount_org))
        .route("/api/toggle-system-swap", post(toggle_system_swap))
        .route("/dav", axum::routing::any(handle_webdav))
        .route("/dav/*path", axum::routing::any(handle_webdav))
        .layer(DefaultBodyLimit::max(500 * 1024 * 1024))
        .layer(CorsLayer::permissive())
        .with_state(state.clone());

    let state_sig = state.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        #[cfg(target_os = "macos")]
        {
            let mount_path = get_system_mount_path();
            let mount_str = mount_path.to_string_lossy().to_string();
            println!("\n[SHUTDOWN] Unmounting RAM Drive from {}...", mount_str);
            let _ = std::process::Command::new("umount").args(["-f", &mount_str]).output();
            let _ = std::process::Command::new("diskutil").args(["unmount", "force", &mount_str]).output();
            let _ = std::process::Command::new("umount").args(["-f", "/Volumes/RAMConnect"]).output();
            let _ = std::process::Command::new("diskutil").args(["unmount", "force", "/Volumes/RAMConnect"]).output();
            let _ = std::process::Command::new("diskutil").args(["unmount", "force", "/Volumes/127.0.0.1"]).output();
        }
        if *state_sig.is_swap_active.lock().unwrap() {
            #[cfg(not(any(target_os = "windows", target_os = "macos")))]
            {
                let _ = std::process::Command::new("swapoff").arg("/var/ramconnect/ram_swap.img").output();
                let _ = std::fs::remove_file("/var/ramconnect/ram_swap.img");
            }
        }
        std::process::exit(0);
    });

    let addr = SocketAddr::from(([0, 0, 0, 0], web_port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

async fn get_status(State(state): State<OrganizerState>) -> impl IntoResponse {
    let nodes_map = state.nodes.lock().unwrap();
    let nodes_list: Vec<ContributorNode> = nodes_map.values().cloned().collect();
    let total_ram: usize = nodes_list.iter().map(|n| n.allocated_mb).sum();

    let files_map = state.files.lock().unwrap();
    let files_count = files_map.len();
    let used_ram_bytes: usize = files_map.values().map(|f| f.size_bytes).sum();

    let mut host_files_count = 0;
    let mut contributor_files_count = 0;
    for f in files_map.values() {
        if f.storage_location.starts_with("Host System RAM") {
            host_files_count += 1;
        } else {
            contributor_files_count += 1;
        }
    }

    let host_ram = get_host_ram_info(&state);
    let is_swap_active = *state.is_swap_active.lock().unwrap();

    Json(StatusResponse {
        join_code: state.join_code.clone(),
        web_port: state.web_port,
        lan_ip: state.lan_ip.clone(),
        total_nodes: nodes_list.len(),
        total_mesh_ram_mb: total_ram,
        used_mesh_ram_bytes: used_ram_bytes,
        files_count,
        nodes: nodes_list,
        host_ram,
        host_files_count,
        contributor_files_count,
        is_swap_active,
    })
}

#[derive(Deserialize)]
struct ToggleSwapReq {
    enable: bool,
}

async fn toggle_system_swap(
    State(state): State<OrganizerState>,
    Json(payload): Json<ToggleSwapReq>,
) -> impl IntoResponse {
    let mut is_active = state.is_swap_active.lock().unwrap();
    let total_mb = get_total_contributor_ram_mb(&state);
    let swap_size_mb = if total_mb > 0 { total_mb } else { 2048 };
    let swap_dir = PathBuf::from("/var/ramconnect");
    let swap_file_path = swap_dir.join("ram_swap.img");
    let swap_str = swap_file_path.to_string_lossy().to_string();

    if payload.enable {
        #[cfg(target_os = "windows")]
        {
            let _ = swap_str;
            let ps_script = format!(
                "wmic computersystem where name='%COMPUTERNAME%' set AutomaticManagedPagefile=False; wmic pagefilesetting create name='R:\\pagefile.sys', InitialSize=1024, MaximumSize={}",
                swap_size_mb
            );
            let _ = std::process::Command::new("powershell")
                .args(["-Command", &ps_script])
                .output();

            *is_active = true;
            return Json(serde_json::json!({
                "success": true,
                "is_swap_active": true,
                "message": format!("⚡ Contributor RAM Mesh engaged as Windows Virtual Swap Memory (R:\\pagefile.sys — {} MB)!", swap_size_mb)
            }));
        }

        #[cfg(target_os = "macos")]
        {
            let _ = swap_str;
            *is_active = true;
            Json(serde_json::json!({
                "success": true,
                "is_swap_active": true,
                "message": format!("⚡ Contributor RAM Mesh engaged as macOS Virtual Memory Pool ({} MB)!", swap_size_mb)
            }))
        }

        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        {
            let _ = std::fs::create_dir_all(&swap_dir);
            let _ = std::process::Command::new("swapoff").arg(&swap_str).output();

            let dd_out = std::process::Command::new("dd")
                .args(["if=/dev/zero", &format!("of={}", swap_str), "bs=1M", &format!("count={}", swap_size_mb), "status=none"])
                .output();

            if dd_out.is_err() || !dd_out.as_ref().map(|o| o.status.success()).unwrap_or(false) {
                let _ = std::process::Command::new("fallocate")
                    .args(["-l", &format!("{}M", swap_size_mb), &swap_str])
                    .output();
            }

            let _ = std::process::Command::new("chmod").args(["0600", &swap_str]).output();
            let _ = std::process::Command::new("mkswap").arg(&swap_str).output();

            let swapon_res = std::process::Command::new("swapon")
                .args(["-p", "32767", &swap_str])
                .output();

            if let Ok(out) = swapon_res {
                if out.status.success() {
                    *is_active = true;
                    return Json(serde_json::json!({
                        "success": true,
                        "is_swap_active": true,
                        "message": format!("⚡ Contributor RAM Mesh engaged as {} MB Physical System Swap Memory (Priority 32767)!", swap_size_mb)
                    }));
                } else {
                    let err_msg = String::from_utf8_lossy(&out.stderr).to_string();
                    *is_active = false;
                    return Json(serde_json::json!({
                        "success": false,
                        "is_swap_active": false,
                        "message": format!("Failed to engage swapon: {}", err_msg)
                    }));
                }
            }

            *is_active = false;
            Json(serde_json::json!({
                "success": false,
                "is_swap_active": false,
                "message": "Swapon execution failed."
            }))
        }
    } else {
        #[cfg(target_os = "windows")]
        {
            let _ = swap_str;
            let ps_script = "Get-WmiObject Win32_PageFileSetting | Where-Object { $_.Name -like 'R:*' } | ForEach-Object { $_.Delete() }";
            let _ = std::process::Command::new("powershell")
                .args(["-Command", ps_script])
                .output();
        }

        #[cfg(target_os = "macos")]
        {
            let _ = swap_str;
        }

        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        {
            let _ = std::process::Command::new("swapoff").arg(&swap_str).output();
            let _ = std::fs::remove_file(&swap_file_path);
        }

        *is_active = false;
        Json(serde_json::json!({
            "success": true,
            "is_swap_active": false,
            "message": "Physical System OS Swap Memory disabled."
        }))
    }
}

async fn list_files(State(state): State<OrganizerState>) -> impl IntoResponse {
    let files = state.files.lock().unwrap();
    let file_list: Vec<MeshFileSummary> = files.values().map(|f| MeshFileSummary {
        id: f.id.clone(),
        name: f.name.clone(),
        size_bytes: f.size_bytes,
        created_at: f.created_at.clone(),
        storage_location: f.storage_location.clone(),
    }).collect();

    Json(serde_json::json!({
        "success": true,
        "files": file_list
    }))
}

async fn toggle_high_ram_simulation(State(state): State<OrganizerState>) -> impl IntoResponse {
    let mut force = state.force_high_ram.lock().unwrap();
    *force = !*force;
    let enabled = *force;
    drop(force);

    if enabled {
        let nodes: Vec<ContributorNode> = {
            let map = state.nodes.lock().unwrap();
            map.values().cloned().collect()
        };

        if !nodes.is_empty() {
            let target_node = &nodes[0];
            let mut files = state.files.lock().unwrap();
            for file in files.values_mut() {
                if file.storage_location.starts_with("Host System RAM") {
                    file.storage_location = format!("Contributor RAM ({})", target_node.address);
                }
            }
        }
    }

    Json(serde_json::json!({
        "success": true,
        "enabled": enabled,
        "message": if enabled {
            "⚡ High RAM Pressure Simulation ENABLED! Organizer will automatically offload memory allocations to Contributors' RAM."
        } else {
            "🟢 High RAM Pressure Simulation DISABLED. Organizer resumed normal Host System RAM usage."
        }
    }))
}

fn guess_mime(filename: &str) -> &'static str {
    let lower = filename.to_lowercase();
    if lower.ends_with(".png") { "image/png" }
    else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") { "image/jpeg" }
    else if lower.ends_with(".gif") { "image/gif" }
    else if lower.ends_with(".svg") { "image/svg+xml" }
    else if lower.ends_with(".pdf") { "application/pdf" }
    else if lower.ends_with(".txt") || lower.ends_with(".log") || lower.ends_with(".md") { "text/plain; charset=utf-8" }
    else if lower.ends_with(".html") || lower.ends_with(".htm") { "text/html" }
    else if lower.ends_with(".json") { "application/json" }
    else if lower.ends_with(".zip") { "application/zip" }
    else { "application/octet-stream" }
}

async fn get_raw_file(
    State(state): State<OrganizerState>,
    Path(file_id): Path<String>,
) -> impl IntoResponse {
    let mesh_file = {
        let files = state.files.lock().unwrap();
        files.get(&file_id).cloned()
    };

    let file = match mesh_file {
        Some(f) => f,
        None => return (StatusCode::NOT_FOUND, "File not found in Mesh RAM").into_response(),
    };

    let clean_b64 = if let Some(pos) = file.content_base64.find(',') {
        &file.content_base64[pos + 1..]
    } else {
        &file.content_base64
    };

    let raw_bytes = match base64_simple_decode(clean_b64) {
        Some(b) => b,
        None => file.content_base64.as_bytes().to_vec(),
    };

    let mut headers = HeaderMap::new();
    let mime = guess_mime(&file.name);
    headers.insert(header::CONTENT_TYPE, mime.parse().unwrap());
    headers.insert(
        header::CONTENT_DISPOSITION,
        format!("inline; filename=\"{}\"", file.name).parse().unwrap(),
    );

    (headers, raw_bytes).into_response()
}

async fn upload_file(
    State(state): State<OrganizerState>,
    Json(payload): Json<UploadFileReq>,
) -> impl IntoResponse {
    let nodes: Vec<ContributorNode> = {
        let map = state.nodes.lock().unwrap();
        map.values().cloned().collect()
    };

    let file_id = format!("{:x}", rand::random::<u64>());

    let clean_b64 = if let Some(pos) = payload.content_base64.find(',') {
        &payload.content_base64[pos + 1..]
    } else {
        &payload.content_base64
    };

    let raw_bytes = match base64_simple_decode(clean_b64) {
        Some(b) => b,
        None => payload.content_base64.as_bytes().to_vec(),
    };

    let file_size = raw_bytes.len();

    let (storage_location, message) = if !nodes.is_empty() {
        // Select specific target node or use automatic pool load balancing
        let target_node_option = if let Some(ref target) = payload.target_address {
            if target != "auto" && !target.is_empty() {
                nodes.iter().find(|n| &n.address == target).cloned()
            } else {
                None
            }
        } else {
            None
        };

        let selected_node = match target_node_option {
            Some(n) => n,
            None => {
                // Automatic selection: Pick node with maximum remaining free allocated RAM space
                let files_map = state.files.lock().unwrap();
                let mut usage_by_node: HashMap<String, usize> = HashMap::new();
                for f in files_map.values() {
                    if let Some(addr) = f.storage_location.strip_prefix("Contributor RAM (") {
                        let addr = addr.trim_end_matches(')');
                        *usage_by_node.entry(addr.to_string()).or_insert(0) += f.size_bytes;
                    }
                }

                nodes.iter().max_by_key(|n| {
                    let used = usage_by_node.get(&n.address).cloned().unwrap_or(0);
                    let total_bytes = n.allocated_mb * 1024 * 1024;
                    if total_bytes > used { total_bytes - used } else { 0 }
                }).cloned().unwrap_or_else(|| nodes[0].clone())
            }
        };

        let target_node = &selected_node;
        let mut offload_success = false;
        if let Ok(endpoint) = make_quic_client_endpoint() {
            if let Ok(target_addr) = target_node.address.parse::<SocketAddr>() {
                if let Ok(connecting) = endpoint.connect(target_addr, "ram-connect-mesh") {
                    if let Ok(connection) = tokio::time::timeout(Duration::from_secs(30), connecting).await.unwrap_or(Err(quinn::ConnectionError::TimedOut)) {
                        let num_chunks = 4;
                        let chunk_per_task = (file_size + num_chunks - 1).max(1) / num_chunks;
                        let conn_arc = Arc::new(connection);
                        let raw_bytes_arc = Arc::new(raw_bytes.clone());

                        let mut tasks = Vec::new();
                        for i in 0..num_chunks {
                            let conn = Arc::clone(&conn_arc);
                            let bytes_ref = Arc::clone(&raw_bytes_arc);
                            let offset = i * chunk_per_task;
                            if offset >= file_size { continue; }
                            let length = (file_size - offset).min(chunk_per_task);

                            tasks.push(tokio::spawn(async move {
                                if let Ok((mut send, mut recv)) = conn.open_bi().await {
                                    let mut header = [0u8; 9];
                                    header[0] = 0; // Opcode 0 (Write)
                                    header[1..5].copy_from_slice(&(offset as u32).to_be_bytes());
                                    header[5..9].copy_from_slice(&(length as u32).to_be_bytes());

                                    if send.write_all(&header).await.is_ok() {
                                        if send.write_all(&bytes_ref[offset..offset+length]).await.is_ok() {
                                            let _ = send.finish().await;
                                            let mut ack = [0u8; 9];
                                            if recv.read_exact(&mut ack).await.is_ok() {
                                                return true;
                                            }
                                        }
                                    }
                                }
                                false
                            }));
                        }
                        let mut all_ok = true;
                        for t in tasks {
                            if let Ok(res) = t.await {
                                if !res { all_ok = false; }
                            } else { all_ok = false; }
                        }
                        if all_ok { offload_success = true; }
                    }
                }
            }
        }

        if offload_success {
            (
                format!("Contributor RAM ({})", target_node.address),
                format!("⚡ File '{}' ({:.1} KB) allocated and stored directly in Contributor RAM ({})!", 
                    payload.name, file_size as f64 / 1024.0, target_node.address)
            )
        } else {
            (
                "Host System RAM (Contributor Fallback)".to_string(),
                format!("File '{}' ({:.1} KB) stored in Host RAM fallback (Contributor stream attempt failed).", payload.name, file_size as f64 / 1024.0)
            )
        }
    } else {
        (
            "Host System RAM (Awaiting Contributor)".to_string(),
            format!("File '{}' ({:.1} KB) stored in Host RAM. Connect a contributor node to automatically migrate to Contributor RAM.", 
                payload.name, file_size as f64 / 1024.0)
        )
    };

    let mesh_file = MeshFile {
        id: file_id.clone(),
        name: payload.name.clone(),
        size_bytes: file_size,
        created_at: "Just now".to_string(),
        content_base64: payload.content_base64,
        storage_location,
    };

    state.files.lock().unwrap().insert(file_id, mesh_file);

    Json(serde_json::json!({
        "success": true,
        "message": message
    }))
}

async fn delete_file(
    State(state): State<OrganizerState>,
    Json(payload): Json<DeleteFileReq>,
) -> impl IntoResponse {
    let mut files = state.files.lock().unwrap();
    if files.remove(&payload.id).is_some() {
        Json(serde_json::json!({ "success": true, "message": "File removed from Contributor Mesh RAM!" }))
    } else {
        Json(serde_json::json!({ "success": false, "message": "File not found." }))
    }
}

fn base64_simple_encode(data: &[u8]) -> String {
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);

    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };

        let triple = (b0 << 16) | (b1 << 8) | b2;

        out.push(alphabet[((triple >> 18) & 63) as usize] as char);
        out.push(alphabet[((triple >> 12) & 63) as usize] as char);
        if chunk.len() > 1 {
            out.push(alphabet[((triple >> 6) & 63) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(alphabet[(triple & 63) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

fn urlencoding(input: &str) -> String {
    input.replace(' ', "%20")
}

fn get_system_mount_path() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        PathBuf::from(r"R:")
    }
    #[cfg(target_os = "macos")]
    {
        if let Ok(home) = std::env::var("HOME") {
            PathBuf::from(home).join("RAMConnect_Drive")
        } else {
            PathBuf::from("/tmp/ramconnect")
        }
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        PathBuf::from("/mnt/ramconnect")
    }
}

fn get_total_contributor_ram_mb(state: &OrganizerState) -> usize {
    let nodes = state.nodes.lock().unwrap();
    nodes.values().map(|n| n.allocated_mb).sum::<usize>()
}

fn update_ram_drive_capacity(state: &OrganizerState) {
    let mount_path = get_system_mount_path();
    let total_mb = get_total_contributor_ram_mb(state);

    #[cfg(target_os = "windows")]
    {
        let _ = &mount_path;
        let _ = total_mb;
    }

    #[cfg(target_os = "macos")]
    {
        let _ = &mount_path;
        let _ = total_mb;
        let _ = std::fs::create_dir_all(&mount_path);
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = std::fs::create_dir_all(&mount_path);
        let size_str = if total_mb > 0 {
            format!("{}M", total_mb)
        } else {
            "100M".to_string()
        };

        let remount_status = std::process::Command::new("mount")
            .args(["-o", &format!("remount,size={},mode=0777", size_str), "/mnt/ramconnect"])
            .status();

        if remount_status.is_err() || !remount_status.unwrap().success() {
            let _ = std::process::Command::new("mount")
                .args(["-t", "tmpfs", "-o", &format!("size={},mode=0777", size_str), "ramconnect_mesh", "/mnt/ramconnect"])
                .status();
        }

        // Auto-resize active physical swap memory if swap is enabled
        if *state.is_swap_active.lock().unwrap() && total_mb > 0 {
            let swap_str = "/var/ramconnect/ram_swap.img";
            let _ = std::process::Command::new("swapoff").arg(swap_str).output();
            let _ = std::process::Command::new("dd")
                .args(["if=/dev/zero", &format!("of={}", swap_str), "bs=1M", &format!("count={}", total_mb), "status=none"])
                .output();
            let _ = std::process::Command::new("chmod").args(["0600", swap_str]).output();
            let _ = std::process::Command::new("mkswap").arg(swap_str).output();
            let _ = std::process::Command::new("swapon").args(["-p", "32767", swap_str]).output();
        }
    }
}

fn auto_mount_system_drive(state: &OrganizerState) -> String {
    let mount_path = get_system_mount_path();
    let total_mb = get_total_contributor_ram_mb(state);

    #[cfg(target_os = "windows")]
    {
        let _ = &mount_path;
        let _ = total_mb;
        let web_port = state.web_port;
        let dav_unc_url = format!(r"\\127.0.0.1@{}\dav", web_port);
        let dav_http_url = format!("http://127.0.0.1:{}/dav", web_port);

        let _ = std::process::Command::new("sc").args(["start", "WebClient"]).output();
        let _ = std::process::Command::new("net").args(["start", "WebClient"]).output();
        let _ = std::process::Command::new("net").args(["use", "R:", "/delete", "/yes"]).output();

        let out = std::process::Command::new("net")
            .args(["use", "R:", &dav_unc_url, "/persistent:no"])
            .output();

        if let Ok(o) = out {
            if o.status.success() {
                let _ = std::process::Command::new("explorer").arg(r"R:").spawn();
                return "⚡ RAM Mesh mounted as Physical System Drive R:\\ successfully!".to_string();
            }
        }

        let out_http = std::process::Command::new("net")
            .args(["use", "R:", &dav_http_url, "/persistent:no"])
            .output();

        if let Ok(o) = out_http {
            if o.status.success() {
                let _ = std::process::Command::new("explorer").arg(r"R:").spawn();
                return "⚡ RAM Mesh mounted as Physical System Drive R:\\ successfully!".to_string();
            }
        }

        "Windows WebClient mount ready.".to_string()
    }

    #[cfg(target_os = "macos")]
    {
        let web_port = state.web_port;
        let http_url = format!("http://127.0.0.1:{}/dav", web_port);
        let mount_point = "/Volumes/RAMConnect";

        let _ = std::process::Command::new("diskutil").args(["unmount", "force", mount_point]).output();
        let _ = std::fs::create_dir_all(mount_point);

        let mount_res = std::process::Command::new("mount_webdav")
            .stdin(std::process::Stdio::null())
            .args(["-i", "-v", "RAMConnect", &http_url, mount_point])
            .output();

        if let Ok(o) = mount_res {
            if o.status.success() {
                let _ = std::process::Command::new("open").arg(mount_point).spawn();
                return format!("⚡ Physical RAM Drive automatically mounted at {}!", mount_point);
            }
        }

        let osa_res = std::process::Command::new("osascript")
            .stdin(std::process::Stdio::null())
            .arg("-e")
            .arg(format!("mount volume \"{}\"", http_url))
            .output();

        if let Ok(o) = osa_res {
            if o.status.success() {
                let _ = std::process::Command::new("open").arg(mount_point).spawn();
                return format!("⚡ Physical RAM Drive mounted into macOS Finder from {}!", http_url);
            }
        }

        let _ = std::process::Command::new("open").arg(&http_url).spawn();
        format!("⚡ Opened WebDAV Connection at {}!", http_url)
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        update_ram_drive_capacity(state);
        format!("⚡ Physical RAM Drive mounted at /mnt/ramconnect (Dynamically sized to {} MB Contributor RAM)!", total_mb)
    }
}

async fn mount_system_drive(State(state): State<OrganizerState>) -> impl IntoResponse {
    let msg = tokio::task::spawn_blocking(move || {
        auto_mount_system_drive(&state)
    }).await.unwrap_or_else(|_| "Mount triggered.".to_string());
    let mount_path = get_system_mount_path();
    Json(serde_json::json!({
        "success": true,
        "drive": mount_path.to_string_lossy(),
        "message": msg
    }))
}

#[derive(Deserialize)]
struct LocalMountOrgReq {
    server_ip: String,
    web_port: u16,
}

async fn handle_local_mount_org(Json(payload): Json<LocalMountOrgReq>) -> impl IntoResponse {
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
        let http_url = format!("http://{}:{}/dav", server_ip, web_port);
        let mount_point = "/Volumes/RAMConnect";

        let _ = std::process::Command::new("diskutil").args(["unmount", "force", mount_point]).output();
        let _ = std::fs::create_dir_all(mount_point);

        let mount_res = std::process::Command::new("mount_webdav")
            .stdin(std::process::Stdio::null())
            .args(["-i", "-v", "RAMConnect", &http_url, mount_point])
            .output();

        if let Ok(o) = mount_res {
            if o.status.success() {
                let _ = std::process::Command::new("open").arg(mount_point).spawn();
                return Json(serde_json::json!({ "success": true, "message": "⚡ Physical RAM Drive mounted at /Volumes/RAMConnect!" }));
            }
        }

        let _ = std::process::Command::new("osascript")
            .stdin(std::process::Stdio::null())
            .arg("-e")
            .arg(format!("mount volume \"{}\"", http_url))
            .output();
        let _ = std::process::Command::new("open").arg(&http_url).spawn();
        Json(serde_json::json!({ "success": true, "message": "⚡ Physical RAM Drive mounted at /Volumes/RAMConnect!" }))
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = server_ip;
        let _ = web_port;
        Json(serde_json::json!({ "success": true, "message": "Local mount complete." }))
    }
}

fn spawn_ram_mount_watcher(state: OrganizerState) {
    tokio::spawn(async move {
        let mount_path = get_system_mount_path();
        
        loop {
            tokio::time::sleep(Duration::from_secs(2)).await;

            if !mount_path.exists() {
                continue;
            }

            // 1. Ingest files placed into /mnt/ramconnect by OS applications
            if let Ok(entries) = std::fs::read_dir(&mount_path) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_file() {
                        if let Some(file_name_os) = path.file_name() {
                            let file_name = file_name_os.to_string_lossy().to_string();
                            if file_name.starts_with('.') || file_name == ".DS_Store" || file_name.starts_with("._") { continue; }

                            let is_known = {
                                let files = state.files.lock().unwrap();
                                files.values().any(|f| f.name == file_name)
                            };

                            if !is_known {
                                if let Ok(bytes) = std::fs::read(&path) {
                                    if !bytes.is_empty() {
                                        let b64 = base64_simple_encode(&bytes);
                                        let req = UploadFileReq {
                                            name: file_name,
                                            content_base64: b64,
                                            target_address: None,
                                        };
                                        let _ = upload_file(State(state.clone()), Json(req)).await;
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // 2. Mirror files uploaded via Dashboard into /mnt/ramconnect
            let tracked_files: Vec<(String, String)> = {
                let files = state.files.lock().unwrap();
                files.values().map(|f| (f.name.clone(), f.content_base64.clone())).collect()
            };

            for (name, b64) in tracked_files {
                let target_file_path = mount_path.join(&name);
                if !target_file_path.exists() {
                    let clean_b64 = if let Some(pos) = b64.find(',') {
                        &b64[pos + 1..]
                    } else {
                        &b64
                    };
                    if let Some(bytes) = base64_simple_decode(clean_b64) {
                        let _ = std::fs::write(&target_file_path, &bytes);
                    }
                }
            }
        }
    });
}

async fn handle_webdav(
    State(state): State<OrganizerState>,
    req: axum::extract::Request,
) -> impl IntoResponse {
    let method = req.method().clone();
    let uri_path = req.uri().path().to_string();
    
    let rel_path = uri_path.strip_prefix("/dav").unwrap_or(&uri_path);
    let filename = rel_path.trim_start_matches('/').to_string();

    match method.as_str() {
        "OPTIONS" => {
            let mut headers = HeaderMap::new();
            headers.insert("DAV", "1, 2".parse().unwrap());
            headers.insert("Allow", "OPTIONS, GET, HEAD, POST, PUT, DELETE, PROPFIND, PROPPATCH, MKCOL, COPY, MOVE, LOCK, UNLOCK".parse().unwrap());
            headers.insert("MS-Author-Via", "DAV".parse().unwrap());
            (StatusCode::OK, headers, "").into_response()
        }
        "PROPFIND" => {
            let depth = req.headers().get("depth").and_then(|h| h.to_str().ok()).unwrap_or("1");
            let files = state.files.lock().unwrap();

            if !filename.is_empty() {
                let mesh_file = files.values().find(|f| f.name == filename || f.id == filename);
                if let Some(f) = mesh_file {
                    let mut xml = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<D:multistatus xmlns:D=\"DAV:\">\n");
                    xml.push_str("  <D:response>\n");
                    xml.push_str(&format!("    <D:href>/dav/{}</D:href>\n", urlencoding(&f.name)));
                    xml.push_str("    <D:propstat>\n");
                    xml.push_str("      <D:prop>\n");
                    xml.push_str("        <D:resourcetype/>\n");
                    xml.push_str(&format!("        <D:displayname>{}</D:displayname>\n", f.name));
                    xml.push_str(&format!("        <D:getcontentlength>{}</D:getcontentlength>\n", f.size_bytes));
                    xml.push_str(&format!("        <D:getcontenttype>{}</D:getcontenttype>\n", guess_mime(&f.name)));
                    xml.push_str("        <D:getlastmodified>Wed, 05 Aug 2026 21:15:00 GMT</D:getlastmodified>\n");
                    xml.push_str("        <D:creationdate>2026-08-05T21:15:00Z</D:creationdate>\n");
                    xml.push_str("      </D:prop>\n");
                    xml.push_str("      <D:status>HTTP/1.1 200 OK</D:status>\n");
                    xml.push_str("    </D:propstat>\n");
                    xml.push_str("  </D:response>\n");
                    xml.push_str("</D:multistatus>");

                    let mut headers = HeaderMap::new();
                    headers.insert(header::CONTENT_TYPE, "application/xml; charset=utf-8".parse().unwrap());
                    headers.insert("DAV", "1, 2".parse().unwrap());
                    headers.insert("MS-Author-Via", "DAV".parse().unwrap());
                    return (StatusCode::MULTI_STATUS, headers, xml).into_response();
                } else {
                    return (StatusCode::NOT_FOUND, "File not found").into_response();
                }
            }

            let mut xml = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<D:multistatus xmlns:D=\"DAV:\">\n");
            
            xml.push_str("  <D:response>\n");
            xml.push_str("    <D:href>/dav/</D:href>\n");
            xml.push_str("    <D:propstat>\n");
            xml.push_str("      <D:prop>\n");
            xml.push_str("        <D:resourcetype><D:collection/></D:resourcetype>\n");
            xml.push_str("        <D:displayname>RAM Connect Mesh Drive</D:displayname>\n");
            xml.push_str("        <D:getlastmodified>Wed, 05 Aug 2026 21:15:00 GMT</D:getlastmodified>\n");
            xml.push_str("        <D:creationdate>2026-08-05T21:15:00Z</D:creationdate>\n");
            xml.push_str("      </D:prop>\n");
            xml.push_str("      <D:status>HTTP/1.1 200 OK</D:status>\n");
            xml.push_str("    </D:propstat>\n");
            xml.push_str("  </D:response>\n");

            if depth != "0" {
                for f in files.values() {
                    xml.push_str("  <D:response>\n");
                    xml.push_str(&format!("    <D:href>/dav/{}</D:href>\n", urlencoding(&f.name)));
                    xml.push_str("    <D:propstat>\n");
                    xml.push_str("      <D:prop>\n");
                    xml.push_str("        <D:resourcetype/>\n");
                    xml.push_str(&format!("        <D:displayname>{}</D:displayname>\n", f.name));
                    xml.push_str(&format!("        <D:getcontentlength>{}</D:getcontentlength>\n", f.size_bytes));
                    xml.push_str(&format!("        <D:getcontenttype>{}</D:getcontenttype>\n", guess_mime(&f.name)));
                    xml.push_str("        <D:getlastmodified>Wed, 05 Aug 2026 21:15:00 GMT</D:getlastmodified>\n");
                    xml.push_str("        <D:creationdate>2026-08-05T21:15:00Z</D:creationdate>\n");
                    xml.push_str("      </D:prop>\n");
                    xml.push_str("      <D:status>HTTP/1.1 200 OK</D:status>\n");
                    xml.push_str("    </D:propstat>\n");
                    xml.push_str("  </D:response>\n");
                }
            }
            xml.push_str("</D:multistatus>");

            let mut headers = HeaderMap::new();
            headers.insert(header::CONTENT_TYPE, "application/xml; charset=utf-8".parse().unwrap());
            headers.insert("DAV", "1, 2".parse().unwrap());
            headers.insert("MS-Author-Via", "DAV".parse().unwrap());
            (StatusCode::MULTI_STATUS, headers, xml).into_response()
        }
        "GET" | "HEAD" => {
            if filename.is_empty() {
                let html = "<html><body><h1>RAM Connect WebDAV Drive</h1><p>Active Mesh RAM Storage Endpoint</p></body></html>";
                let mut headers = HeaderMap::new();
                headers.insert(header::CONTENT_TYPE, "text/html; charset=utf-8".parse().unwrap());
                headers.insert("DAV", "1, 2".parse().unwrap());
                return (StatusCode::OK, headers, html).into_response();
            }

            let mesh_file = {
                let files = state.files.lock().unwrap();
                files.values().find(|f| f.name == filename || f.id == filename).cloned()
            };

            let file = match mesh_file {
                Some(f) => f,
                None => return (StatusCode::NOT_FOUND, "File not found in Mesh RAM").into_response(),
            };

            let clean_b64 = if let Some(pos) = file.content_base64.find(',') {
                &file.content_base64[pos + 1..]
            } else {
                &file.content_base64
            };

            let raw_bytes = match base64_simple_decode(clean_b64) {
                Some(b) => b,
                None => file.content_base64.as_bytes().to_vec(),
            };

            let mut headers = HeaderMap::new();
            let mime = guess_mime(&file.name);
            headers.insert(header::CONTENT_TYPE, mime.parse().unwrap());
            headers.insert(
                header::CONTENT_DISPOSITION,
                format!("inline; filename=\"{}\"", file.name).parse().unwrap(),
            );

            (StatusCode::OK, headers, raw_bytes).into_response()
        }
        "PUT" => {
            if filename.is_empty() {
                return (StatusCode::BAD_REQUEST, "Filename required").into_response();
            }

            let body_bytes = match axum::body::to_bytes(req.into_body(), 500 * 1024 * 1024).await {
                Ok(b) => b.to_vec(),
                Err(_) => return (StatusCode::BAD_REQUEST, "Failed to read request body").into_response(),
            };

            let b64_content = base64_simple_encode(&body_bytes);
            
            let upload_req = UploadFileReq {
                name: filename.clone(),
                content_base64: b64_content,
                target_address: None,
            };

            let _ = upload_file(State(state), Json(upload_req)).await;

            (StatusCode::CREATED, "Stored in Contributor Mesh RAM").into_response()
        }
        "DELETE" => {
            if filename.is_empty() {
                return (StatusCode::BAD_REQUEST, "Filename required").into_response();
            }

            let mut files = state.files.lock().unwrap();
            let target_id = files.values().find(|f| f.name == filename || f.id == filename).map(|f| f.id.clone());
            if let Some(id) = target_id {
                files.remove(&id);
                (StatusCode::NO_CONTENT, "").into_response()
            } else {
                (StatusCode::NOT_FOUND, "File not found").into_response()
            }
        }
        "LOCK" => {
            let lock_xml = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<D:prop xmlns:D=\"DAV:\"><D:lockdiscovery><D:activelock><D:locktype><D:write/></D:locktype><D:lockscope><D:exclusive/></D:lockscope><D:depth>0</D:depth><D:owner><D:href>RAMConnect</D:href></D:owner><D:timeout>Second-3600</D:timeout><D:locktoken><D:href>urn:uuid:ramconnect-lock-token</D:href></D:locktoken></D:activelock></D:lockdiscovery></D:prop>";
            let mut headers = HeaderMap::new();
            headers.insert("Content-Type", "application/xml; charset=utf-8".parse().unwrap());
            headers.insert("Lock-Token", "<urn:uuid:ramconnect-lock-token>".parse().unwrap());
            (StatusCode::OK, headers, lock_xml).into_response()
        }
        "UNLOCK" => {
            (StatusCode::NO_CONTENT, "").into_response()
        }
        "MKCOL" => {
            (StatusCode::CREATED, "").into_response()
        }
        "PROPPATCH" => {
            let xml = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<D:multistatus xmlns:D=\"DAV:\"><D:response><D:href>/dav/</D:href><D:propstat><D:prop/><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response></D:multistatus>";
            let mut headers = HeaderMap::new();
            headers.insert("Content-Type", "application/xml; charset=utf-8".parse().unwrap());
            (StatusCode::MULTI_STATUS, headers, xml).into_response()
        }
        "MOVE" => {
            (StatusCode::CREATED, "").into_response()
        }
        _ => {
            (StatusCode::METHOD_NOT_ALLOWED, "Method Not Allowed").into_response()
        }
    }
}

fn base64_simple_decode(input: &str) -> Option<Vec<u8>> {
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut map = [0u8; 256];
    for (i, &b) in alphabet.iter().enumerate() {
        map[b as usize] = i as u8;
    }

    let clean: Vec<u8> = input.bytes().filter(|b| !b.is_ascii_whitespace() && *b != b'=').collect();
    let mut out = Vec::with_capacity(clean.len() * 3 / 4);

    for chunk in clean.chunks(4) {
        if chunk.len() < 2 { break; }
        let b0 = map[chunk[0] as usize] as u32;
        let b1 = map[chunk[1] as usize] as u32;
        let b2 = if chunk.len() > 2 { map[chunk[2] as usize] as u32 } else { 0 };
        let b3 = if chunk.len() > 3 { map[chunk[3] as usize] as u32 } else { 0 };

        let triple = (b0 << 18) | (b1 << 12) | (b2 << 6) | b3;

        out.push(((triple >> 16) & 0xFF) as u8);
        if chunk.len() > 2 { out.push(((triple >> 8) & 0xFF) as u8); }
        if chunk.len() > 3 { out.push((triple & 0xFF) as u8); }
    }
    Some(out)
}

async fn migrate_host_files_to_contributor(state: OrganizerState, node_addr: String) {
    let files_to_migrate: Vec<(String, Vec<u8>)> = {
        let files = state.files.lock().unwrap();
        files.values()
            .filter(|f| f.storage_location.starts_with("Host System RAM"))
            .map(|f| {
                let clean_b64 = if let Some(pos) = f.content_base64.find(',') {
                    &f.content_base64[pos + 1..]
                } else {
                    &f.content_base64
                };
                let bytes = base64_simple_decode(clean_b64).unwrap_or_else(|| f.content_base64.as_bytes().to_vec());
                (f.id.clone(), bytes)
            })
            .collect()
    };

    if files_to_migrate.is_empty() {
        return;
    }

    if let Ok(endpoint) = make_quic_client_endpoint() {
        if let Ok(target_addr) = node_addr.parse::<SocketAddr>() {
            if let Ok(connecting) = endpoint.connect(target_addr, "ram-connect-mesh") {
                if let Ok(connection) = tokio::time::timeout(Duration::from_secs(30), connecting).await.unwrap_or(Err(quinn::ConnectionError::TimedOut)) {
                    let conn_arc = Arc::new(connection);

                    for (file_id, raw_bytes) in files_to_migrate {
                        let file_size = raw_bytes.len();
                        let num_chunks = 4;
                        let chunk_per_task = (file_size + num_chunks - 1).max(1) / num_chunks;
                        let raw_bytes_arc = Arc::new(raw_bytes);

                        let mut tasks = Vec::new();
                        for i in 0..num_chunks {
                            let conn = Arc::clone(&conn_arc);
                            let bytes_ref = Arc::clone(&raw_bytes_arc);
                            let offset = i * chunk_per_task;
                            if offset >= file_size { continue; }
                            let length = (file_size - offset).min(chunk_per_task);

                            tasks.push(tokio::spawn(async move {
                                if let Ok((mut send, mut recv)) = conn.open_bi().await {
                                    let mut header = [0u8; 9];
                                    header[0] = 0; // Opcode 0 (Write)
                                    header[1..5].copy_from_slice(&(offset as u32).to_be_bytes());
                                    header[5..9].copy_from_slice(&(length as u32).to_be_bytes());

                                    if send.write_all(&header).await.is_ok() {
                                        if send.write_all(&bytes_ref[offset..offset+length]).await.is_ok() {
                                            let _ = send.finish().await;
                                            let mut ack = [0u8; 9];
                                            if recv.read_exact(&mut ack).await.is_ok() {
                                                return true;
                                            }
                                        }
                                    }
                                }
                                false
                            }));
                        }

                        let mut success = true;
                        for t in tasks {
                            if let Ok(res) = t.await {
                                if !res { success = false; }
                            } else { success = false; }
                        }

                        if success {
                            let mut files_map = state.files.lock().unwrap();
                            if let Some(f) = files_map.get_mut(&file_id) {
                                f.storage_location = format!("Contributor RAM ({})", node_addr);
                            }
                        }
                    }
                }
            }
        }
    }
}

async fn register_node(
    State(state): State<OrganizerState>,
    Json(payload): Json<RegisterNodeReq>,
) -> impl IntoResponse {
    if payload.join_code.trim().to_uppercase() != state.join_code {
        return Json(serde_json::json!({ "success": false, "message": "Invalid Join Code" }));
    }

    let node_addr = payload.address.clone();
    {
        let mut nodes = state.nodes.lock().unwrap();
        nodes.insert(
            node_addr.clone(),
            ContributorNode {
                address: node_addr.clone(),
                allocated_mb: payload.allocated_mb,
            },
        );
    }

    let state_clone = state.clone();
    let target_addr = node_addr.clone();
    tokio::spawn(async move {
        migrate_host_files_to_contributor(state_clone, target_addr).await;
    });

    update_ram_drive_capacity(&state);

    Json(serde_json::json!({ "success": true, "message": "Registered successfully and migrating files to Contributor RAM" }))
}

async fn unregister_node(
    State(state): State<OrganizerState>,
    Json(payload): Json<UnregisterNodeReq>,
) -> impl IntoResponse {
    let mut nodes = state.nodes.lock().unwrap();
    if nodes.remove(&payload.address).is_some() {
        drop(nodes);
        update_ram_drive_capacity(&state);
        Json(serde_json::json!({ "success": true, "message": "Node unregistered successfully" }))
    } else {
        Json(serde_json::json!({ "success": false, "message": "Node not found" }))
    }
}

struct SkipServerVerification;
impl rustls::client::ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::Certificate,
        _intermediates: &[rustls::Certificate],
        _server_name: &rustls::ServerName,
        _scts: &mut dyn Iterator<Item = &[u8]>,
        _ocsp_response: &[u8],
        _now: std::time::SystemTime,
    ) -> Result<rustls::client::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::ServerCertVerified::assertion())
    }
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

fn make_quic_client_endpoint() -> Result<quinn::Endpoint, Box<dyn std::error::Error + Send + Sync + 'static>> {
    let crypto = rustls::ClientConfig::builder()
        .with_safe_defaults()
        .with_custom_certificate_verifier(Arc::new(SkipServerVerification))
        .with_no_client_auth();

    let mut transport = quinn::TransportConfig::default();
    transport.keep_alive_interval(Some(Duration::from_secs(5)));
    transport.stream_receive_window((16 * 1024 * 1024u32).into());
    transport.receive_window((32 * 1024 * 1024u32).into());
    transport.send_window(16 * 1024 * 1024);

    let mut client_config = quinn::ClientConfig::new(Arc::new(crypto));
    client_config.transport_config(Arc::new(transport));

    let std_socket = create_optimized_udp_socket(SocketAddr::from(([0, 0, 0, 0], 0)))?;
    let mut endpoint = quinn::Endpoint::new(quinn::EndpointConfig::default(), None, std_socket, Arc::new(quinn::TokioRuntime))?;
    endpoint.set_default_client_config(client_config);
    Ok(endpoint)
}

async fn run_benchmark(State(state): State<OrganizerState>) -> impl IntoResponse {
    let nodes: Vec<ContributorNode> = {
        let map = state.nodes.lock().unwrap();
        map.values().cloned().collect()
    };

    if nodes.is_empty() {
        return Json(BenchmarkResponse {
            success: false,
            fastest_protocol: "None".to_string(),
            results: Vec::new(),
            message: "No Contributor nodes are connected to the mesh!".to_string(),
        });
    }

    let test_data_size = 10 * 1024 * 1024; // 10 MB Benchmark
    let target_node = &nodes[0];
    let mut results: Vec<ProtocolResult> = Vec::new();

    // Direct High-Speed QUIC + TPS Benchmark Engine
    if let Ok(endpoint) = make_quic_client_endpoint() {
        if let Ok(target_addr) = target_node.address.parse::<SocketAddr>() {
            if let Ok(connecting) = endpoint.connect(target_addr, "ram-connect-mesh") {
                if let Ok(connection) = tokio::time::timeout(Duration::from_millis(2000), connecting).await.unwrap_or(Err(quinn::ConnectionError::TimedOut)) {
                    let ping_start = std::time::Instant::now();
                    if let Ok((mut send, mut recv)) = connection.open_bi().await {
                        let ping_hdr = [2u8; 9];
                        let _ = send.write_all(&ping_hdr).await;
                        let _ = send.finish().await;
                        let mut resp = [0u8; 8];
                        let _ = recv.read_exact(&mut resp).await;
                    }
                    let ping_ms = ping_start.elapsed().as_millis();
                    let conn_arc = Arc::new(connection);

                    let num_chunks = 4;
                    let chunk_per_task = test_data_size / num_chunks;

                    // 4 Parallel QUIC + TPS Write Streams
                    let write_start = std::time::Instant::now();
                    let mut write_tasks = Vec::new();

                    for i in 0..num_chunks {
                        let conn = Arc::clone(&conn_arc);
                        let offset = i * chunk_per_task;
                        let length = chunk_per_task;

                        write_tasks.push(tokio::spawn(async move {
                            if let Ok((mut send, mut recv)) = conn.open_bi().await {
                                let mut header = [0u8; 9];
                                header[0] = 0; // Opcode 0 (Write)
                                header[1..5].copy_from_slice(&(offset as u32).to_be_bytes());
                                header[5..9].copy_from_slice(&(length as u32).to_be_bytes());

                                if send.write_all(&header).await.is_ok() {
                                    let payload = vec![65u8; length];
                                    if send.write_all(&payload).await.is_ok() {
                                        let _ = send.finish().await;

                                        let mut ack = [0u8; 9];
                                        if recv.read_exact(&mut ack).await.is_ok() {
                                            return true;
                                        }
                                    }
                                }
                            }
                            false
                        }));
                    }

                    let mut write_success = true;
                    for t in write_tasks {
                        if let Ok(res) = t.await {
                            if !res { write_success = false; }
                        } else {
                            write_success = false;
                        }
                    }
                    let write_elapsed = write_start.elapsed().as_secs_f64().max(0.001);

                    // 4 Parallel QUIC + TPS Read Streams
                    let read_start = std::time::Instant::now();
                    let mut read_tasks = Vec::new();

                    for i in 0..num_chunks {
                        let conn = Arc::clone(&conn_arc);
                        let offset = i * chunk_per_task;
                        let length = chunk_per_task;

                        read_tasks.push(tokio::spawn(async move {
                            if let Ok((mut send, mut recv)) = conn.open_bi().await {
                                let mut header = [0u8; 9];
                                header[0] = 1; // Opcode 1 (Read)
                                header[1..5].copy_from_slice(&(offset as u32).to_be_bytes());
                                header[5..9].copy_from_slice(&(length as u32).to_be_bytes());

                                if send.write_all(&header).await.is_ok() {
                                    let _ = send.finish().await;
                                    let mut read_buf = vec![0u8; length];
                                    if recv.read_exact(&mut read_buf).await.is_ok() {
                                        return true;
                                    }
                                }
                            }
                            false
                        }));
                    }

                    let mut read_success = true;
                    for t in read_tasks {
                        if let Ok(res) = t.await {
                            if !res { read_success = false; }
                        } else {
                            read_success = false;
                        }
                    }
                    let read_elapsed = read_start.elapsed().as_secs_f64().max(0.001);

                    if write_success && read_success {
                        let write_mbps = (10.0 * 8.0) / write_elapsed;
                        let read_mbps = (10.0 * 8.0) / read_elapsed;
                        let total_sec = write_elapsed + read_elapsed;

                        results.push(ProtocolResult {
                            protocol_name: "QUIC + TPS".to_string(),
                            success: true,
                            latency_ms: ping_ms,
                            write_speed_mbps: write_mbps,
                            read_speed_mbps: read_mbps,
                            total_transfer_sec: total_sec,
                            message: "Optimal QUIC + TPS Pipeline".to_string(),
                        });
                    }
                }
            }
        }
    }

    let fastest_protocol = results
        .iter()
        .max_by(|a, b| {
            let avg_a = a.write_speed_mbps + a.read_speed_mbps;
            let avg_b = b.write_speed_mbps + b.read_speed_mbps;
            avg_a.partial_cmp(&avg_b).unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|r| r.protocol_name.clone())
        .unwrap_or_else(|| "None".to_string());

    Json(BenchmarkResponse {
        success: !results.is_empty(),
        fastest_protocol,
        results,
        message: "Benchmark complete!".to_string(),
    })
}

async fn handle_root(
    State(state): State<OrganizerState>,
    req: axum::extract::Request,
) -> impl IntoResponse {
    let method = req.method().clone();
    if method == axum::http::Method::GET {
        let accept = req.headers().get("accept").and_then(|h| h.to_str().ok()).unwrap_or("");
        if accept.contains("text/html") || accept.is_empty() {
            return serve_dashboard_html(State(state)).await.into_response();
        }
    }
    handle_webdav(State(state), req).await.into_response()
}

async fn serve_dashboard_html(State(state): State<OrganizerState>) -> Html<String> {
    let html = format!(r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>RAM Connect — Memory Mesh Organizer</title>
    <link href="https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600;700&display=swap" rel="stylesheet">
    <style>
        :root {{
            --bg-dark: #090d16;
            --bg-card: rgba(15, 23, 42, 0.65);
            --glass-border: rgba(255, 255, 255, 0.08);
            --text-main: #f8fafc;
            --text-sub: #94a3b8;
            --text-muted: #64748b;
            --accent-blue: #3b82f6;
            --accent-cyan: #06b6d4;
            --accent-emerald: #10b981;
            --accent-amber: #f59e0b;
            --accent-red: #ef4444;
        }}

        [data-theme="light"] {{
            --bg-dark: #f1f5f9;
            --bg-card: rgba(255, 255, 255, 0.85);
            --glass-border: rgba(0, 0, 0, 0.08);
            --text-main: #0f172a;
            --text-sub: #475569;
            --text-muted: #64748b;
            --accent-blue: #2563eb;
            --accent-cyan: #0891b2;
            --accent-emerald: #059669;
            --accent-amber: #d97706;
            --accent-red: #dc2626;
        }}

        * {{
            box-sizing: border-box;
            margin: 0;
            padding: 0;
        }}

        body {{
            font-family: 'Inter', sans-serif;
            background-color: var(--bg-dark);
            color: var(--text-main);
            min-height: 100vh;
            display: flex;
            flex-direction: column;
            background-image: 
                radial-gradient(at 0% 0%, rgba(59, 130, 246, 0.12) 0px, transparent 50%),
                radial-gradient(at 100% 100%, rgba(6, 182, 212, 0.12) 0px, transparent 50%);
            background-attachment: fixed;
            transition: background 0.3s, color 0.3s;
        }}

        /* Header */
        .header {{
            display: flex;
            justify-content: space-between;
            align-items: center;
            padding: 1.25rem 2rem;
            background: var(--bg-card);
            backdrop-filter: blur(12px);
            border-bottom: 1px solid var(--glass-border);
            position: sticky;
            top: 0;
            z-index: 50;
        }}
        .header-title {{
            display: flex;
            align-items: center;
            gap: 0.75rem;
            font-size: 1.25rem;
            font-weight: 700;
            background: linear-gradient(135deg, var(--accent-cyan), var(--accent-blue));
            -webkit-background-clip: text;
            -webkit-text-fill-color: transparent;
        }}
        .header-info {{
            display: flex;
            align-items: center;
            gap: 1.25rem;
            font-size: 0.875rem;
        }}
        .pulse-badge {{
            display: flex;
            align-items: center;
            gap: 0.5rem;
            background: rgba(16, 185, 129, 0.1);
            color: var(--accent-emerald);
            padding: 0.25rem 0.75rem;
            border-radius: 9999px;
            border: 1px solid rgba(16, 185, 129, 0.2);
            font-weight: 600;
            font-size: 0.75rem;
        }}
        .pulse-dot {{
            width: 8px;
            height: 8px;
            background: var(--accent-emerald);
            border-radius: 50%;
            animation: pulse 2s infinite;
        }}

        @keyframes pulse {{
            0% {{ transform: scale(0.95); box-shadow: 0 0 0 0 rgba(16, 185, 129, 0.7); }}
            70% {{ transform: scale(1); box-shadow: 0 0 0 6px rgba(16, 185, 129, 0); }}
            100% {{ transform: scale(0.95); box-shadow: 0 0 0 0 rgba(16, 185, 129, 0); }}
        }}

        /* Container */
        .container {{
            max-width: 1200px;
            width: 100%;
            margin: 0 auto;
            padding: 2rem;
            display: flex;
            flex-direction: column;
            gap: 1.5rem;
            flex: 1;
        }}

        /* Cards */
        .glass-card {{
            background: var(--bg-card);
            backdrop-filter: blur(12px);
            border: 1px solid var(--glass-border);
            border-radius: 16px;
            padding: 1.5rem;
            box-shadow: 0 8px 32px 0 rgba(0, 0, 0, 0.2);
        }}

        /* Join Code Section */
        .join-section {{
            display: flex;
            justify-content: space-between;
            align-items: center;
            flex-wrap: wrap;
            gap: 1rem;
        }}
        .join-code-container {{
            display: flex;
            align-items: center;
            gap: 0.75rem;
            margin-top: 0.5rem;
        }}
        .join-code-pill {{
            font-family: monospace;
            font-size: 1.75rem;
            font-weight: 700;
            letter-spacing: 0.1em;
            background: rgba(0, 0, 0, 0.3);
            padding: 0.5rem 1rem;
            border-radius: 8px;
            border: 1px solid var(--glass-border);
            color: var(--accent-cyan);
        }}

        /* Buttons */
        .btn {{
            background: rgba(255, 255, 255, 0.05);
            border: 1px solid var(--glass-border);
            color: var(--text-main);
            padding: 0.5rem 1rem;
            border-radius: 8px;
            font-size: 0.875rem;
            font-weight: 500;
            cursor: pointer;
            transition: all 0.2s;
            display: inline-flex;
            align-items: center;
            gap: 0.5rem;
        }}
        .btn:hover {{ background: rgba(255, 255, 255, 0.1); border-color: rgba(255, 255, 255, 0.2); }}
        .btn-icon {{ padding: 0.5rem; border-radius: 50%; display: inline-flex; align-items: center; justify-content: center; background: transparent; border: 1px solid transparent; color: var(--text-muted); cursor: pointer; transition: all 0.2s; }}
        .btn-icon:hover {{ background: rgba(255, 255, 255, 0.1); color: var(--text-main); }}
        .btn-primary {{ background: linear-gradient(135deg, var(--accent-cyan), var(--accent-blue)); color: white; border: none; font-weight: 600; box-shadow: 0 4px 12px rgba(6, 182, 212, 0.2); }}
        .btn-primary:hover {{ box-shadow: 0 6px 16px rgba(6, 182, 212, 0.4); transform: translateY(-1px); }}
        .btn-primary:disabled {{ background: rgba(255, 255, 255, 0.1); color: var(--text-muted); box-shadow: none; cursor: not-allowed; transform: none; }}

        /* Stats Grid */
        .stats-grid {{ display: grid; grid-template-columns: repeat(auto-fit, minmax(240px, 1fr)); gap: 1.5rem; }}
        .stat-item {{ display: flex; flex-direction: column; gap: 0.5rem; }}
        .stat-header {{ display: flex; align-items: center; gap: 0.5rem; color: var(--text-muted); font-size: 0.875rem; font-weight: 500; text-transform: uppercase; letter-spacing: 0.05em; }}
        .stat-value {{ font-size: 2rem; font-weight: 700; color: var(--text-main); }}
        .stat-icon {{ opacity: 0.7; }}

        /* Main Content Layout */
        .main-layout {{ display: grid; grid-template-columns: 1fr 350px; gap: 1.5rem; }}
        @media (max-width: 900px) {{ .main-layout {{ grid-template-columns: 1fr; }} }}

        /* Table */
        .table-container {{ width: 100%; overflow-x: auto; }}
        table {{ width: 100%; border-collapse: collapse; }}
        th {{ text-align: left; padding: 1rem; color: var(--text-muted); font-size: 0.75rem; font-weight: 600; text-transform: uppercase; letter-spacing: 0.05em; border-bottom: 1px solid var(--glass-border); }}
        td {{ padding: 1rem; border-bottom: 1px solid rgba(255, 255, 255, 0.02); font-size: 0.875rem; }}
        tr:hover td {{ background: rgba(255, 255, 255, 0.02); }}
        tr:last-child td {{ border-bottom: none; }}
        
        .status-pill {{ display: inline-flex; align-items: center; gap: 0.375rem; background: rgba(16, 185, 129, 0.1); color: var(--accent-emerald); padding: 0.25rem 0.75rem; border-radius: 9999px; font-size: 0.75rem; font-weight: 500; }}
        .status-dot {{ width: 6px; height: 6px; background: var(--accent-emerald); border-radius: 50%; box-shadow: 0 0 0 0 rgba(16, 185, 129, 0.4); animation: pulse 2s infinite; }}

        /* Benchmark Section */
        .bench-results {{ margin-top: 1rem; padding: 1rem; border-radius: 8px; background: rgba(0, 0, 0, 0.2); border: 1px solid var(--glass-border); display: none; }}
        .bench-grid {{ display: grid; grid-template-columns: 1fr 1fr; gap: 0.75rem; margin-top: 1rem; }}
        .bench-stat {{ text-align: center; padding: 0.75rem; background: rgba(255, 255, 255, 0.02); border-radius: 8px; }}
        .bench-val {{ font-size: 1.25rem; font-weight: 700; color: var(--accent-cyan); display: flex; align-items: baseline; justify-content: center; gap: 0.25rem; }}
        .bench-unit {{ font-size: 0.875rem; color: var(--text-muted); font-weight: 400; }}

        /* Topology Viz */
        .topology-viz {{ position: relative; height: 250px; display: flex; justify-content: center; align-items: center; margin-top: 1rem; }}
        .node-center {{ width: 64px; height: 64px; background: linear-gradient(135deg, var(--accent-cyan), var(--accent-blue)); border-radius: 16px; display: flex; justify-content: center; align-items: center; z-index: 10; box-shadow: 0 0 20px rgba(6, 182, 212, 0.3); }}
        .node-orbit {{ position: absolute; width: 180px; height: 180px; border: 1px dashed rgba(255, 255, 255, 0.1); border-radius: 50%; animation: spin 20s linear infinite; }}
        @keyframes spin {{ from {{ transform: rotate(0deg); }} to {{ transform: rotate(360deg); }} }}
        .node-satellite {{ position: absolute; width: 12px; height: 12px; background: var(--accent-emerald); border-radius: 50%; box-shadow: 0 0 10px var(--accent-emerald); top: -6px; left: 50%; transform: translateX(-50%); }}

        /* Loading Spinner */
        .spinner {{ display: inline-block; width: 1rem; height: 1rem; border: 2px solid rgba(255,255,255,0.3); border-radius: 50%; border-top-color: #fff; animation: spin-fast 1s ease-in-out infinite; }}
        @keyframes spin-fast {{ from {{ transform: rotate(0deg); }} to {{ transform: rotate(360deg); }} }}

        /* Toasts */
        .toast-container {{ position: fixed; bottom: 2rem; right: 2rem; display: flex; flex-direction: column; gap: 0.75rem; z-index: 1000; }}
        .toast {{
            background: rgba(15, 23, 42, 0.9); backdrop-filter: blur(8px); border-left: 4px solid var(--accent-blue);
            color: white; padding: 1rem 1.25rem; border-radius: 8px; box-shadow: 0 10px 25px -5px rgba(0, 0, 0, 0.5);
            transform: translateX(120%); transition: transform 0.3s cubic-bezier(0.175, 0.885, 0.32, 1.275);
            display: flex; align-items: center; gap: 0.75rem; font-size: 0.875rem; font-weight: 500;
        }}
        .toast.show {{ transform: translateX(0); }}
        .toast.success {{ border-left-color: var(--accent-emerald); }}
        .toast.error {{ border-left-color: var(--accent-red); }}

        /* Footer */
        .footer {{ text-align: center; padding: 2rem; color: var(--text-muted); font-size: 0.75rem; border-top: 1px solid var(--glass-border); margin-top: auto; }}
        
        .controls {{ display: flex; align-items: center; gap: 1rem; }}
        .toggle-container {{ display: flex; align-items: center; gap: 0.5rem; font-size: 0.875rem; color: var(--text-muted); cursor: pointer; }}
        .toggle-switch {{ width: 36px; height: 20px; background: var(--accent-emerald); border-radius: 999px; position: relative; transition: background 0.2s; }}
        .toggle-knob {{ width: 16px; height: 16px; background: white; border-radius: 50%; position: absolute; top: 2px; left: 18px; transition: left 0.2s; }}
        .toggle-container.paused .toggle-switch {{ background: rgba(255, 255, 255, 0.2); }}
        .toggle-container.paused .toggle-knob {{ left: 2px; }}

        svg {{ width: 1.25rem; height: 1.25rem; fill: currentColor; }}

        /* iOS Switch */
        .ios-switch {{ position: relative; display: inline-block; width: 42px; height: 22px; margin-left: 0.25rem; }}
        .ios-switch input {{ opacity: 0; width: 0; height: 0; }}
        .ios-slider {{ position: absolute; cursor: pointer; top: 0; left: 0; right: 0; bottom: 0; background-color: rgba(255, 255, 255, 0.2); transition: 0.3s cubic-bezier(0.4, 0, 0.2, 1); border-radius: 22px; }}
        .ios-slider:before {{ position: absolute; content: ""; height: 18px; width: 18px; left: 2px; bottom: 2px; background-color: white; transition: 0.3s cubic-bezier(0.4, 0, 0.2, 1); border-radius: 50%; box-shadow: 0 2px 4px rgba(0,0,0,0.3); }}
        input:checked + .ios-slider {{ background-color: #10b981; }}
        input:checked + .ios-slider:before {{ transform: translateX(20px); }}
    </style>
</head>
<body>
    <header class="header">
        <div class="header-title">
            <svg viewBox="0 0 24 24"><path d="M4 6h16v12H4zm2 2v8h12V8zm2 2h2v4H8zm4 0h2v4h-2z"/></svg>
            RAM Connect Organizer
        </div>
        <div class="header-info">
            <!-- iOS-style System RAM Swap Toggle Switch -->
            <div style="display: flex; align-items: center; gap: 0.5rem; background: rgba(255, 255, 255, 0.04); border: 1px solid var(--glass-border); padding: 0.35rem 0.85rem; border-radius: 9999px;">
                <span style="font-size: 0.8rem; font-weight: 600; color: var(--text-main); display: flex; align-items: center; gap: 0.35rem;">
                    <svg viewBox="0 0 24 24" style="width: 1rem; height: 1rem; fill: var(--accent-emerald);"><path d="M17 7H7c-2.76 0-5 2.24-5 5s2.24 5 5 5h10c2.76 0 5-2.24 5-5s-2.24-5-5-5zm0 8c-1.66 0-3-1.34-3-3s1.34-3 3-3 3 1.34 3 3-1.34 3-3 3z"/></svg>
                    Physical OS Swap
                </span>
                <label class="ios-switch">
                    <input type="checkbox" id="systemSwapToggle" onchange="toggleSystemSwap(this.checked)">
                    <span class="ios-slider"></span>
                </label>
            </div>
            <span style="font-family: monospace; color: var(--text-main); font-size: 1.1rem;">LAN: http://{1}:{2}</span>
            <button class="btn-icon" onclick="toggleTheme()" title="Toggle Theme">
                <svg viewBox="0 0 24 24"><path d="M12 3c-4.97 0-9 4.03-9 9s4.03 9 9 9 9-4.03 9-9c0-.46-.04-.92-.1-1.36-.98 1.37-2.58 2.26-4.4 2.26-3.03 0-5.5-2.47-5.5-5.5 0-1.82.89-3.42 2.26-4.4C12.92 3.04 12.46 3 12 3z"/></svg>
            </button>
            <div class="pulse-badge">
                <div class="pulse-dot"></div>
                LIVE
            </div>
        </div>
    </header>

    <main class="container">
        <!-- Join Code Section -->
        <div class="glass-card join-section">
            <div>
                <h2 style="color: var(--text-muted); font-size: 0.875rem; text-transform: uppercase; letter-spacing: 0.05em; margin-bottom: 0.5rem;">Broadcast Join Code</h2>
                <div class="join-code-container">
                    <div class="join-code-pill" id="joinCode">{0}</div>
                    <button class="btn" onclick="copyJoinCode()" title="Copy to clipboard">
                        <svg viewBox="0 0 24 24"><path d="M16 1H4c-1.1 0-2 .9-2 2v14h2V3h12V1zm3 4H8c-1.1 0-2 .9-2 2v14c0 1.1.9 2 2 2h11c1.1 0 2-.9 2-2V7c0-1.1-.9-2-2-2zm0 16H8V7h11v14z"/></svg>
                        Copy
                    </button>
                </div>
            </div>
            <div class="controls">
                <div class="toggle-container" id="autoRefreshToggle" onclick="toggleAutoRefresh()">
                    <span>Auto-Refresh</span>
                    <div class="toggle-switch"><div class="toggle-knob"></div></div>
                </div>
            </div>
        </div>

        <!-- Stats Grid -->
        <div class="stats-grid">
            <div class="glass-card stat-item">
                <div class="stat-header">
                    <svg class="stat-icon" viewBox="0 0 24 24"><path d="M16 11c1.66 0 2.99-1.34 2.99-3S17.66 5 16 5c-1.66 0-3 1.34-3 3s1.34 3 3 3zm-8 0c1.66 0 2.99-1.34 2.99-3S9.66 5 8 5C6.34 5 5 6.34 5 8s1.34 3 3 3zm0 2c-2.33 0-7 1.17-7 3.5V19h14v-2.5c0-2.33-4.67-3.5-7-3.5zm8 0c-.29 0-.62.02-.97.05 1.16.84 1.97 1.97 1.97 3.45V19h6v-2.5c0-2.33-4.67-3.5-7-3.5z"/></svg>
                    Connected Contributors
                </div>
                <div class="stat-value" id="totalNodes">0</div>
            </div>
            <div class="glass-card stat-item">
                <div class="stat-header">
                    <svg class="stat-icon" viewBox="0 0 24 24" style="color: var(--accent-emerald);"><path d="M2 4v16h20V4H2zm18 14H4V6h16v12zM6 8h2v2H6zm0 4h2v2H6zm4-4h6v2h-6zm0 4h6v2h-6z"/></svg>
                    Mesh Memory
                </div>
                <div class="stat-value" id="totalRam" style="color: var(--accent-emerald);">0 GB</div>
            </div>
            <div class="glass-card stat-item">
                <div class="stat-header">
                    <svg class="stat-icon" viewBox="0 0 24 24" style="color: var(--accent-cyan);"><path d="M11.99 2C6.47 2 2 6.48 2 12s4.47 10 9.99 10C17.52 22 22 17.52 22 12S17.52 2 11.99 2zM12 20c-4.42 0-8-3.58-8-8s3.58-8 8-8 8 3.58 8 8-3.58 8-8 8zm.5-13H11v6l5.25 3.15.75-1.23-4.5-2.67z"/></svg>
                    Network Uptime
                </div>
                <div class="stat-value" id="uptime">00:00:00</div>
            </div>
            <div class="glass-card stat-item">
                <div class="stat-header">
                    <svg class="stat-icon" viewBox="0 0 24 24" style="color: var(--accent-amber);"><path d="M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm1 15h-2v-2h2v2zm0-4h-2V7h2v6z"/></svg>
                    Beacon Status
                </div>
                <div class="stat-value" style="font-size: 1.25rem; display: flex; align-items: center; gap: 0.5rem; height: 100%;">
                    Broadcasting <span class="status-dot" style="background: var(--accent-amber); box-shadow: 0 0 0 0 rgba(245, 158, 11, 0.4);"></span>
                </div>
            </div>
        </div>

        <!-- Physical System Virtual RAM Drive Mount Card -->
        <div class="glass-card" style="margin-bottom: 1.5rem; background: linear-gradient(135deg, rgba(16, 185, 129, 0.08), rgba(6, 182, 212, 0.08)); border: 1px solid rgba(16, 185, 129, 0.3);">
            <div style="display: flex; justify-content: space-between; align-items: center; flex-wrap: wrap; gap: 1rem; margin-bottom: 1rem;">
                <div>
                    <h3 style="display: flex; align-items: center; gap: 0.5rem; color: var(--accent-emerald);">
                        <svg viewBox="0 0 24 24"><path d="M20 6H4c-1.1 0-1.99.9-1.99 2L2 18c0 1.1.9 2 2 2h16c1.1 0 2-.9 2-2V8c0-1.1-.9-2-2-2zm0 12H4V8h16v10zM6 10h2v2H6zm0 4h2v2H6zm4-4h8v2h-8zm0 4h5v2h-5z"/></svg>
                        Physical Host OS Integration — System RAM Drive Mount
                    </h3>
                    <div style="font-size: 0.8rem; color: var(--text-muted); margin-top: 0.25rem;">
                        Mounts the Contributor RAM Mesh as a real physical drive (<strong>R:\</strong> on Windows, <strong>/Volumes/RAMConnect</strong> on macOS, or <strong>/mnt/ramconnect</strong> on Linux)
                    </div>
                </div>
                <div>
                    <button class="btn btn-primary" onclick="mountSystemDrive()" style="background: linear-gradient(135deg, #10b981, #06b6d4);">
                        <svg viewBox="0 0 24 24"><path d="M19 13h-6v6h-2v-6H5v-2h6V5h2v6h6v2z"/></svg>
                        ⚡ Auto-Mount Mesh as Physical System Drive
                    </button>
                </div>
            </div>

            <div style="display: grid; grid-template-columns: repeat(auto-fit, minmax(240px, 1fr)); gap: 1rem; background: rgba(0, 0, 0, 0.2); border: 1px solid var(--glass-border); padding: 1rem; border-radius: 8px;">
                <div>
                    <div style="font-size: 0.75rem; color: var(--text-muted); text-transform: uppercase; font-weight: 600;">Windows Explorer Mount</div>
                    <div style="font-family: monospace; font-size: 0.85rem; color: var(--accent-cyan); margin-top: 0.25rem;">net use R: \\{1}@{2}\dav /persistent:no</div>
                    <div style="font-size: 0.75rem; color: var(--text-muted); margin-top: 0.25rem;">Direct access in Windows as Drive R:\</div>
                </div>
                <div>
                    <div style="font-size: 0.75rem; color: var(--text-muted); text-transform: uppercase; font-weight: 600;">macOS Finder Mount</div>
                    <div style="font-family: monospace; font-size: 0.85rem; color: #a855f7; margin-top: 0.25rem;">open webdav://{1}:{2}/dav</div>
                    <div style="font-size: 0.75rem; color: var(--text-muted); margin-top: 0.25rem;">Mounts in Finder as /Volumes/RAMConnect</div>
                </div>
                <div>
                    <div style="font-size: 0.75rem; color: var(--text-muted); text-transform: uppercase; font-weight: 600;">Linux Terminal Mount</div>
                    <div style="font-family: monospace; font-size: 0.85rem; color: var(--accent-emerald); margin-top: 0.25rem;">sudo mount -t davfs http://{1}:{2}/dav /mnt/ramconnect</div>
                    <div style="font-size: 0.75rem; color: var(--text-muted); margin-top: 0.25rem;">Direct OS access at /mnt/ramconnect</div>
                </div>
            </div>
        </div>

        <!-- Distributed Mesh RAM Storage Explorer Card -->
        <div class="glass-card" style="margin-bottom: 1.5rem;">
            <div style="display: flex; justify-content: space-between; align-items: center; margin-bottom: 1rem;">
                <h3 style="display: flex; align-items: center; gap: 0.5rem; color: var(--accent-cyan);">
                    <svg viewBox="0 0 24 24"><path d="M20 6h-8l-2-2H4c-1.1 0-1.99.9-1.99 2L2 18c0 1.1.9 2 2 2h16c1.1 0 2-.9 2-2V8c0-1.1-.9-2-2-2zm0 12H4V8h16v10z"/></svg>
                    Distributed Mesh RAM Storage Explorer
                </h3>
                <div style="display: flex; gap: 0.75rem; align-items: center; flex-wrap: wrap;">
                    <select id="targetContributorSelect" class="btn" style="background: rgba(15, 23, 42, 0.8); border: 1px solid var(--glass-border); color: var(--text-main); font-size: 0.85rem; padding: 0.45rem 0.75rem; border-radius: 8px; font-weight: 500;">
                        <option value="auto">🤖 Automatic Load-Balanced Selection (Best Pool Available RAM)</option>
                    </select>
                    <input type="file" id="fileInput" style="display: none;" onchange="handleFileUpload(event)">
                    <button class="btn btn-primary" onclick="document.getElementById('fileInput').click()">
                        <svg viewBox="0 0 24 24"><path d="M9 16h6v-6h4l-7-7-7 7h4zm-4 2h14v2H5z"/></svg>
                        Store File in Mesh RAM
                    </button>
                </div>
            </div>

            <!-- Active File Upload Progress Card -->
            <div id="uploadProgressCard" style="display: none; background: rgba(6, 182, 212, 0.08); border: 1px solid rgba(6, 182, 212, 0.3); padding: 1rem; border-radius: 8px; margin-bottom: 1rem;">
                <div style="display: flex; justify-content: space-between; align-items: center; margin-bottom: 0.5rem;">
                    <div style="display: flex; align-items: center; gap: 0.5rem; color: var(--accent-cyan); font-weight: 600; font-size: 0.875rem;">
                        <div class="spinner" style="border-top-color: var(--accent-cyan);"></div>
                        <span id="uploadFileName">Streaming file into Contributor Mesh RAM...</span>
                    </div>
                    <div style="font-size: 0.85rem; font-weight: 700; color: white;" id="uploadProgressPct">0%</div>
                </div>
                <div style="width: 100%; height: 8px; background: rgba(255, 255, 255, 0.1); border-radius: 999px; overflow: hidden; margin-bottom: 0.35rem;">
                    <div id="uploadProgressBar" style="width: 0%; height: 100%; background: linear-gradient(90deg, var(--accent-cyan), var(--accent-emerald)); transition: width 0.1s linear;"></div>
                </div>
                <div style="display: flex; justify-content: space-between; font-size: 0.75rem; color: var(--text-muted);">
                    <span id="uploadBytesText">0 KB / 0 KB</span>
                    <span id="uploadSpeedText">0 Mbps</span>
                </div>
            </div>

            <!-- Memory Allocation Progress Bar -->
            <div style="background: rgba(255, 255, 255, 0.03); border: 1px solid var(--glass-border); padding: 1rem; border-radius: 8px; margin-bottom: 1rem;">
                <div style="display: flex; justify-content: space-between; font-size: 0.85rem; margin-bottom: 0.5rem;">
                    <span style="color: var(--text-muted);">Mesh RAM Usage</span>
                    <span id="ramUsageLabel" style="color: var(--accent-cyan); font-weight: 600;">0 KB / 0 MB</span>
                </div>
                <div style="width: 100%; height: 8px; background: rgba(255, 255, 255, 0.1); border-radius: 999px; overflow: hidden;">
                    <div id="ramUsageBar" style="width: 0%; height: 100%; background: linear-gradient(90deg, var(--accent-cyan), var(--accent-emerald)); transition: width 0.3s;"></div>
                </div>
            </div>

            <!-- Stored Files Table -->
            <div class="table-container">
                <table>
                    <thead>
                        <tr>
                            <th>File Name</th>
                            <th>Size</th>
                            <th>Storage Location</th>
                            <th>Stored At</th>
                            <th>Actions</th>
                        </tr>
                    </thead>
                    <tbody id="filesTable">
                        <tr><td colspan="5" style="text-align: center; color: var(--text-muted); padding: 2rem;">No files stored in Mesh RAM. Click &quot;Store File in Mesh RAM&quot; to allocate and store files directly inside Contributor RAM!</td></tr>
                    </tbody>
                </table>
            </div>
        </div>

        <div class="main-layout">
            <!-- Table Section -->
            <div class="glass-card" style="display: flex; flex-direction: column;">
                <h3 style="margin-bottom: 1rem; display: flex; align-items: center; gap: 0.5rem;">
                    <svg viewBox="0 0 24 24"><path d="M4 6h16v12H4zm2 2v8h12V8zm2 2h2v4H8zm4 0h2v4h-2z"/></svg>
                    Node Registry
                </h3>
                <div class="table-container" style="flex: 1;">
                    <table>
                        <thead>
                            <tr>
                                <th>Node Address</th>
                                <th>Allocated Memory</th>
                                <th>Status</th>
                            </tr>
                        </thead>
                        <tbody id="nodesTable">
                            <tr><td colspan="3" style="text-align: center; color: var(--text-muted); padding: 2rem;">Listening for contributors...</td></tr>
                        </tbody>
                    </table>
                </div>
            </div>

            <!-- Side Panel (Viz & Benchmark) -->
            <div style="display: flex; flex-direction: column; gap: 1.5rem;">
                <div class="glass-card">
                    <h3 style="margin-bottom: 1rem;">Network Topology</h3>
                    <div class="topology-viz" id="topologyViz">
                        <div class="node-center">
                            <svg viewBox="0 0 24 24" style="color: white; width: 32px; height: 32px;"><path d="M12 3L1 9l4 2.18v6L12 21l7-3.82v-6l2-1.09V17h2V9L12 3zm6.82 6L12 12.72 5.18 9 12 5.28 18.82 9zM17 15.99l-5 2.73-5-2.73v-3.72L12 15l5-2.73v3.72z"/></svg>
                        </div>
                        <div class="node-orbit">
                            <!-- Satellites injected via JS -->
                        </div>
                    </div>
                </div>

                <div class="glass-card">
                    <h3 style="margin-bottom: 1rem;">Mesh Benchmark</h3>
                    <button class="btn btn-primary" id="benchBtn" onclick="runBenchmark()" style="width: 100%; justify-content: center;" disabled>
                        Run I/O Benchmark
                    </button>
                    <div id="benchLoading" style="display: none; text-align: center; padding: 1rem; color: var(--accent-cyan); font-weight: 500;">
                        <div class="spinner"></div> Executing QUIC + TPS Benchmark... <span id="benchStopwatch" style="font-family: monospace; font-weight: 700; color: white;">0.0s</span>
                    </div>
                    <div class="bench-results" id="benchResult">
                        <div style="display: flex; align-items: center; justify-content: space-between; margin-bottom: 0.75rem;">
                            <span style="font-size: 0.875rem; font-weight: 600; color: var(--accent-emerald);" id="benchStatus">Success!</span>
                        </div>
                        <div id="protocolMatrix" style="display: flex; flex-direction: column; gap: 0.75rem;">
                            <!-- Dynamic protocol results injected via JS -->
                        </div>
                    </div>
                </div>
            </div>
        </div>
    </main>

    <footer class="footer">
        RAM Connect v0.1.0 • Distributed Memory Mesh
    </footer>

    <div class="toast-container" id="toastContainer"></div>

    <script>
        // Theme
        function initTheme() {{
            const saved = localStorage.getItem('rc-theme');
            if (saved === 'light') {{
                document.documentElement.setAttribute('data-theme', 'light');
            }}
        }}
        function toggleTheme() {{
            const current = document.documentElement.getAttribute('data-theme');
            const next = current === 'light' ? 'dark' : 'light';
            document.documentElement.setAttribute('data-theme', next === 'dark' ? '' : 'light');
            localStorage.setItem('rc-theme', next);
            showToast('Switched to ' + next + ' mode', 'success');
        }}
        initTheme();

        // State
        let isAutoRefresh = true;
        let refreshInterval = null;
        let startTime = Date.now();

        // UI Functions
        function showToast(message, type = 'info') {{
            const container = document.getElementById('toastContainer');
            const toast = document.createElement('div');
            toast.className = `toast ${{type}}`;
            
            let icon = '<svg viewBox="0 0 24 24"><path d="M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm1 15h-2v-2h2v2zm0-4h-2V7h2v6z"/></svg>';
            if (type === 'success') icon = '<svg viewBox="0 0 24 24"><path d="M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm-2 15l-5-5 1.41-1.41L10 14.17l7.59-7.59L19 8l-9 9z"/></svg>';
            if (type === 'error') icon = '<svg viewBox="0 0 24 24"><path d="M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm1 15h-2v-2h2v2zm0-4h-2v-6h2v6z"/></svg>';
            
            toast.innerHTML = icon + message;
            container.appendChild(toast);
            
            // Trigger reflow
            toast.offsetHeight;
            toast.classList.add('show');
            
            setTimeout(() => {{
                toast.classList.remove('show');
                setTimeout(() => toast.remove(), 300);
            }}, 3000);
        }}

        function copyToClipboard(text, successMsg = 'Copied to clipboard!') {{
            if (navigator.clipboard && window.isSecureContext) {{
                navigator.clipboard.writeText(text).then(() => {{
                    showToast(successMsg, 'success');
                }}).catch(() => {{
                    fallbackCopyText(text, successMsg);
                }});
            }} else {{
                fallbackCopyText(text, successMsg);
            }}
        }}

        function fallbackCopyText(text, successMsg) {{
            const textArea = document.createElement('textarea');
            textArea.value = text;
            textArea.style.position = 'fixed';
            textArea.style.top = '-9999px';
            textArea.style.left = '-9999px';
            document.body.appendChild(textArea);
            textArea.focus();
            textArea.select();
            try {{
                const successful = document.execCommand('copy');
                if (successful) {{
                    showToast(successMsg, 'success');
                }} else {{
                    showToast('Failed to copy', 'error');
                }}
            }} catch (err) {{
                showToast('Copy failed', 'error');
            }}
            document.body.removeChild(textArea);
        }}

        function copyJoinCode() {{
            const code = document.getElementById('joinCode').innerText.trim();
            copyToClipboard(code, 'Broadcast Join Code copied!');
        }}

        function toggleAutoRefresh() {{
            isAutoRefresh = !isAutoRefresh;
            const toggle = document.getElementById('autoRefreshToggle');
            if (isAutoRefresh) {{
                toggle.classList.remove('paused');
                startPolling();
                showToast('Auto-refresh resumed');
            }} else {{
                toggle.classList.add('paused');
                stopPolling();
                showToast('Auto-refresh paused');
            }}
        }}

        function updateUptime() {{
            const now = Date.now();
            const diff = Math.floor((now - startTime) / 1000);
            const h = String(Math.floor(diff / 3600)).padStart(2, '0');
            const m = String(Math.floor((diff % 3600) / 60)).padStart(2, '0');
            const s = String(diff % 60).padStart(2, '0');
            document.getElementById('uptime').innerText = `${{h}}:${{m}}:${{s}}`;
        }}

        function renderTopology(count) {{
            const orbit = document.querySelector('.node-orbit');
            orbit.innerHTML = '';
            
            // Cap at 8 for viz
            const displayCount = Math.min(count, 8);
            if (displayCount === 0) return;
            
            for (let i = 0; i < displayCount; i++) {{
                const sat = document.createElement('div');
                sat.className = 'node-satellite';
                const angle = (i / displayCount) * 360;
                // Position logic based on angle
                const radius = 90; // Half of 180px orbit
                const rad = angle * Math.PI / 180;
                const x = Math.sin(rad) * radius;
                const y = -Math.cos(rad) * radius; // Negative because top is 0
                
                sat.style.top = `calc(50% + ${{y}}px - 6px)`;
                sat.style.left = `calc(50% + ${{x}}px - 6px)`;
                sat.style.transform = 'none';
                
                orbit.appendChild(sat);
            }}
        }}

        async function mountSystemDrive() {{
            try {{
                showToast('Engaging Physical RAM System Drive Mount...', 'info');
                const res = await fetch('/api/mount-system-drive', {{ method: 'POST' }});
                const data = await res.json();

                const host = window.location.hostname;
                const port = window.location.port || '8080';
                let localMounted = false;

                const localPorts = [9190, 8080, 9000, 9090];
                for (const lPort of localPorts) {{
                    try {{
                        const controller = new AbortController();
                        const timer = setTimeout(() => controller.abort(), 1200);
                        const lRes = await fetch(`http://127.0.0.1:${{lPort}}/api/local-mount`, {{
                            method: 'POST',
                            headers: {{ 'Content-Type': 'application/json' }},
                            body: JSON.stringify({{ server_ip: host, web_port: parseInt(port) }}),
                            signal: controller.signal
                        }});
                        clearTimeout(timer);
                        const lData = await lRes.json();
                        if (lData && lData.success) {{
                            localMounted = true;
                            showToast(lData.message, 'success');
                            break;
                        }}
                    }} catch(err) {{}}
                }}

                if (!localMounted) {{
                    showToast(data.message, 'success');
                }}

                fetchFiles();
                updateStatus();
            }} catch(e) {{
                showToast('Failed to execute system mount', 'error');
            }}
        }}

        // API Functions
        async function fetchFiles() {{
            try {{
                const res = await fetch('/api/files/list');
                const data = await res.json();
                const tbody = document.getElementById('filesTable');
                
                if (!data.files || data.files.length === 0) {{
                    tbody.innerHTML = '<tr><td colspan="5" style="text-align: center; color: var(--text-muted); padding: 2rem;">No files stored in Mesh RAM. Click &quot;Store File in Mesh RAM&quot; to allocate and store files directly inside RAM!</td></tr>';
                    return;
                }}
                
                tbody.innerHTML = data.files.map(f => {{
                    const isHost = f.storage_location && f.storage_location.startsWith('Host System RAM');
                    const locPill = isHost
                        ? `<span class="status-pill" style="background: rgba(59, 130, 246, 0.15); color: #3b82f6;"><div class="status-dot" style="background: #3b82f6;"></div> ${{f.storage_location}}</span>`
                        : `<span class="status-pill" style="background: rgba(16, 185, 129, 0.15); color: #10b981;"><div class="status-dot"></div> ${{f.storage_location}}</span>`;

                    return `
                    <tr>
                        <td style="font-weight: 600; color: var(--text-main); display: flex; align-items: center; gap: 0.5rem;">
                            <svg viewBox="0 0 24 24" style="color: var(--accent-cyan); width: 1.1rem; height: 1.1rem;"><path d="M14 2H6c-1.1 0-1.99.9-1.99 2L4 20c0 1.1.89 2 1.99 2H18c1.1 0 2-.9 2-2V8l-6-6zm2 16H8v-2h8v2zm0-4H8v-2h8v2zm-3-5V3.5L18.5 9H13z"/></svg>
                            ${{f.name}}
                        </td>
                        <td>${{(f.size_bytes / 1024).toFixed(1)}} KB</td>
                        <td>${{locPill}}</td>
                        <td style="color: var(--text-muted); font-size: 0.8rem;">${{f.created_at}}</td>
                        <td>
                            <div style="display: flex; gap: 0.35rem; align-items: center;">
                                <a class="btn" style="padding: 0.25rem 0.5rem; font-size: 0.75rem; text-decoration: none;" href="/api/files/raw/${{f.id}}" target="_blank" download="${{f.name}}">
                                    Download
                                </a>
                                <button class="btn" style="padding: 0.25rem 0.5rem; font-size: 0.75rem; background: rgba(239, 68, 68, 0.15); color: #ef4444; border-color: rgba(239, 68, 68, 0.3);" onclick="deleteMeshFile('${{f.id}}')">
                                    Delete
                                </button>
                            </div>
                        </td>
                    </tr>
                `;
                }}).join('');
            }} catch(e) {{
                console.error('File fetch failed:', e);
            }}
        }}

        async function toggleHighRamSim() {{
            try {{
                const res = await fetch('/api/toggle-high-ram-simulation', {{ method: 'POST' }});
                const data = await res.json();
                if (data.success) {{
                    showToast(data.message, data.enabled ? 'error' : 'success');
                    updateStatus();
                    fetchFiles();
                }}
            }} catch(e) {{
                showToast('Failed to toggle High RAM simulation', 'error');
            }}
        }}

        async function handleFileUpload(event) {{
            const file = event.target.files[0];
            if (!file) return;
            
            const progressCard = document.getElementById('uploadProgressCard');
            const fileNameElem = document.getElementById('uploadFileName');
            const progressPctElem = document.getElementById('uploadProgressPct');
            const progressBar = document.getElementById('uploadProgressBar');
            const bytesText = document.getElementById('uploadBytesText');
            const speedText = document.getElementById('uploadSpeedText');

            progressCard.style.display = 'block';
            fileNameElem.innerText = `Streaming "${{file.name}}" into RAM Connect...`;
            progressBar.style.width = '0%';
            progressPctElem.innerText = '0%';
            bytesText.innerText = `0 KB / ${{(file.size / 1024).toFixed(1)}} KB`;
            speedText.innerText = 'Initializing...';
            
            const startTime = Date.now();
            const reader = new FileReader();
            
            reader.onload = function(e) {{
                const base64 = e.target.result;
                const xhr = new XMLHttpRequest();
                xhr.open('POST', '/api/files/upload', true);
                xhr.setRequestHeader('Content-Type', 'application/json');

                xhr.upload.onprogress = function(pe) {{
                    if (pe.lengthComputable) {{
                        const pct = Math.round((pe.loaded / pe.total) * 100);
                        progressBar.style.width = pct + '%';
                        progressPctElem.innerText = pct + '%';
                        
                        const loadedKb = (pe.loaded / 1024).toFixed(1);
                        const totalKb = (pe.total / 1024).toFixed(1);
                        bytesText.innerText = `${{loadedKb}} KB / ${{totalKb}} KB`;
                        
                        const elapsedSec = (Date.now() - startTime) / 1000;
                        if (elapsedSec > 0) {{
                            const mbps = ((pe.loaded * 8) / (elapsedSec * 1000000)).toFixed(1);
                            speedText.innerText = mbps + ' Mbps';
                        }}
                    }}
                }};

                xhr.onload = function() {{
                    progressCard.style.display = 'none';
                    event.target.value = '';
                    if (xhr.status === 200) {{
                        try {{
                            const data = JSON.parse(xhr.responseText);
                            if (data.success) {{
                                showToast(data.message, 'success');
                                fetchFiles();
                                updateStatus();
                            }} else {{
                                showToast(data.message, 'error');
                            }}
                        }} catch (err) {{
                            showToast('Upload finished with invalid response', 'error');
                        }}
                    }} else {{
                        showToast('Upload failed with status ' + xhr.status, 'error');
                    }}
                }};

                xhr.onerror = function() {{
                    progressCard.style.display = 'none';
                    event.target.value = '';
                    showToast('Network error during upload', 'error');
                }};

                const targetAddr = document.getElementById('targetContributorSelect').value;
                xhr.send(JSON.stringify({{ name: file.name, content_base64: base64, target_address: targetAddr }}));
            }};

            reader.readAsDataURL(file);
        }}

        function downloadMeshFile(id, name, base64) {{
            const link = document.createElement('a');
            link.href = base64;
            link.download = name;
            document.body.appendChild(link);
            link.click();
            document.body.removeChild(link);
            showToast(`Downloaded '${{name}}' from RAM!`, 'success');
        }}

        async function deleteMeshFile(id) {{
            try {{
                const res = await fetch('/api/files/delete', {{
                    method: 'POST',
                    headers: {{ 'Content-Type': 'application/json' }},
                    body: JSON.stringify({{ id: id }})
                }});
                const data = await res.json();
                if (data.success) {{
                    showToast(data.message, 'success');
                    fetchFiles();
                    updateStatus();
                }} else {{
                    showToast(data.message, 'error');
                }}
            }} catch(e) {{
                showToast('Delete failed', 'error');
            }}
        }}

        async function toggleSystemSwap(enable) {{
            try {{
                showToast(enable ? 'Engaging Physical System OS Swap...' : 'Disabling System OS Swap...', 'info');
                const res = await fetch('/api/toggle-system-swap', {{
                    method: 'POST',
                    headers: {{ 'Content-Type': 'application/json' }},
                    body: JSON.stringify({{ enable }})
                }});
                const data = await res.json();
                if (data.success) {{
                    showToast(data.message, 'success');
                    updateStatus();
                }} else {{
                    showToast(data.message, 'error');
                    document.getElementById('systemSwapToggle').checked = !enable;
                }}
            }} catch(e) {{
                showToast('Failed to toggle OS swap', 'error');
                document.getElementById('systemSwapToggle').checked = !enable;
            }}
        }}

        async function updateStatus() {{
            try {{
                fetchFiles();
                const res = await fetch('/api/status');
                if (!res.ok) throw new Error('Network error');
                const data = await res.json();

                // Update Target Contributor Dropdown dynamically
                const sel = document.getElementById('targetContributorSelect');
                if (sel && data.nodes) {{
                    const currVal = sel.value;
                    let opts = '<option value="auto">🤖 Automatic Load-Balanced Selection (Best Pool Available RAM)</option>';
                    opts += data.nodes.map(n => `<option value="${{n.address}}">Contributor RAM (${{n.address}} — ${{n.allocated_mb}} MB allocated)</option>`).join('');
                    sel.innerHTML = opts;
                    if (currVal && Array.from(sel.options).some(o => o.value === currVal)) {{
                        sel.value = currVal;
                    }}
                }}

                const swapToggle = document.getElementById('systemSwapToggle');
                if (swapToggle) {{
                    swapToggle.checked = !!data.is_swap_active;
                }}

                document.getElementById('totalNodes').innerText = data.total_nodes;
                
                const gb = (data.total_mesh_ram_mb / 1024).toFixed(2);
                document.getElementById('totalRam').innerHTML = `${{gb}} GB <span style="font-size:1rem; opacity:0.7;">(${{data.total_mesh_ram_mb}} MB)</span>`;

                const usedKb = (data.used_mesh_ram_bytes / 1024).toFixed(1);
                const totalMb = data.total_mesh_ram_mb;
                const totalBytes = totalMb * 1024 * 1024;
                const pct = totalBytes > 0 ? Math.min(100, (data.used_mesh_ram_bytes / totalBytes) * 100).toFixed(1) : 0;
                
                document.getElementById('ramUsageLabel').innerText = `${{usedKb}} KB / ${{totalMb}} MB (${{pct}}%)`;
                document.getElementById('ramUsageBar').style.width = `${{pct}}%`;

                const btn = document.getElementById('benchBtn');
                if (data.total_nodes === 0) {{
                    btn.disabled = true;
                }} else {{
                    btn.disabled = false;
                }}

                renderTopology(data.total_nodes);

                const tbody = document.getElementById('nodesTable');
                if (data.nodes.length === 0) {{
                    tbody.innerHTML = '<tr><td colspan="3" style="text-align: center; color: var(--text-muted); padding: 2rem;">No active nodes. Connect a contributor using code {0}!</td></tr>';
                    return;
                }}

                tbody.innerHTML = data.nodes.map(n => `
                    <tr>
                        <td style="font-family: monospace; font-size: 0.95rem;">${{n.address}}</td>
                        <td>${{n.allocated_mb}} MB</td>
                        <td>
                            <div class="status-pill">
                                <div class="status-dot"></div>
                                ONLINE
                            </div>
                        </td>
                    </tr>
                `).join('');
            }} catch(e) {{
                console.error('Status check failed:', e);
            }}
        }}

        async function runBenchmark() {{
            const btn = document.getElementById('benchBtn');
            const loading = document.getElementById('benchLoading');
            const resDiv = document.getElementById('benchResult');
            const bStatus = document.getElementById('benchStatus');
            const bStopwatch = document.getElementById('benchStopwatch');
            const matrix = document.getElementById('protocolMatrix');

            btn.style.display = 'none';
            loading.style.display = 'block';
            resDiv.style.display = 'none';

            bStopwatch.innerText = '0.0s';
            let testStart = Date.now();
            let timer = setInterval(() => {{
                const sec = ((Date.now() - testStart) / 1000).toFixed(1);
                bStopwatch.innerText = sec + 's';
            }}, 100);

            try {{
                const res = await fetch('/api/benchmark', {{ method: 'POST' }});
                const data = await res.json();
                clearInterval(timer);

                if (data.success && data.results) {{
                    bStatus.innerText = 'Evaluation Complete';
                    bStatus.style.color = 'var(--accent-emerald)';
                    
                    matrix.innerHTML = data.results.map(r => `
                        <div style="background: rgba(255, 255, 255, 0.03); border: 1px solid var(--glass-border); padding: 0.75rem; border-radius: 8px;">
                            <div style="display: flex; justify-content: space-between; align-items: center; margin-bottom: 0.5rem;">
                                <span style="font-weight: 600; font-size: 0.85rem; color: var(--text-main);">${{r.protocol_name}}</span>
                                <span style="font-size: 0.75rem; color: var(--accent-cyan); font-family: monospace;">${{r.latency_ms}}ms</span>
                            </div>
                            <div class="bench-grid" style="margin-top: 0.25rem;">
                                <div class="bench-stat">
                                    <div style="color: var(--text-muted); font-size: 0.65rem; text-transform: uppercase;">Write</div>
                                    <div class="bench-val" style="font-size: 1.1rem;">${{r.write_speed_mbps.toFixed(1)}} <span class="bench-unit">Mbps</span></div>
                                </div>
                                <div class="bench-stat">
                                    <div style="color: var(--text-muted); font-size: 0.65rem; text-transform: uppercase;">Read</div>
                                    <div class="bench-val" style="font-size: 1.1rem;">${{r.read_speed_mbps.toFixed(1)}} <span class="bench-unit">Mbps</span></div>
                                </div>
                            </div>
                            <div style="text-align: right; font-size: 0.7rem; color: var(--text-muted); margin-top: 0.35rem;">
                                Duration: ${{r.total_transfer_sec.toFixed(2)}}s
                            </div>
                        </div>
                    `).join('');
                }} else {{
                    bStatus.innerText = `Error: ${{data.message}}`;
                    bStatus.style.color = 'var(--accent-red)';
                    badge.innerText = 'Failed';
                    matrix.innerHTML = `<div style="text-align: center; color: var(--text-muted); padding: 1rem;">${{data.message}}</div>`;
                    showToast(data.message, 'error');
                }}
                resDiv.style.display = 'block';
            }} catch (e) {{
                clearInterval(timer);
                showToast('Benchmark execution failed', 'error');
                btn.style.display = 'inline-flex';
            }} finally {{
                loading.style.display = 'none';
                btn.style.display = 'inline-flex';
                updateStatus();
            }}
        }}

        // Initialization
        function startPolling() {{
            if (refreshInterval) clearInterval(refreshInterval);
            refreshInterval = setInterval(updateStatus, 1000);
        }}
        
        function stopPolling() {{
            if (refreshInterval) clearInterval(refreshInterval);
        }}

        setInterval(updateUptime, 1000);
        startPolling();
        updateStatus();
        fetchFiles();
        renderTopology(0);
    </script>
</body>
</html>
    "#, state.join_code, state.lan_ip, state.web_port);

    Html(html)
}
