use std::env;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::thread;

fn handle_client(mut stream: TcpStream, mountpoint: PathBuf, dashboard_dir: PathBuf) {
    let mut buffer = [0; 1024];
    if let Ok(size) = stream.read(&mut buffer) {
        if size == 0 { return; }
        let request = String::from_utf8_lossy(&buffer[..size]);
        let mut lines = request.lines();
        let request_line = lines.next().unwrap_or("");
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or("");
        let path = parts.next().unwrap_or("/");

        if method == "GET" {
            if path == "/api/telemetry" {
                let telemetry_path = mountpoint.join(".vexfs-telemetry.json");
                if let Ok(content) = fs::read_to_string(&telemetry_path) {
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nAccess-Control-Allow-Origin: *\r\n\r\n{}",
                        content
                    );
                    let _ = stream.write_all(response.as_bytes());
                } else {
                    let response = "HTTP/1.1 500 Internal Server Error\r\n\r\n{}";
                    let _ = stream.write_all(response.as_bytes());
                }
            } else {
                // Serve static files
                let file_path = if path == "/" {
                    dashboard_dir.join("index.html")
                } else {
                    let relative_path = path.trim_start_matches('/');
                    dashboard_dir.join(relative_path)
                };

                if file_path.exists() && file_path.is_file() {
                    if let Ok(content) = fs::read(&file_path) {
                        let content_type = if path.ends_with(".css") {
                            "text/css"
                        } else if path.ends_with(".js") {
                            "application/javascript"
                        } else {
                            "text/html"
                        };
                        let response_header = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\n\r\n",
                            content_type,
                            content.len()
                        );
                        let mut response = response_header.into_bytes();
                        response.extend(content);
                        let _ = stream.write_all(&response);
                    }
                } else {
                    let response = "HTTP/1.1 404 Not Found\r\n\r\n404 Not Found";
                    let _ = stream.write_all(response.as_bytes());
                }
            }
        }
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: vexfs_daemon <mountpoint> [port]");
        std::process::exit(1);
    }

    let mountpoint = PathBuf::from(&args[1]);
    let port = if args.len() > 2 { &args[2] } else { "8080" };
    let dashboard_dir = env::current_dir().unwrap().join("dashboard");

    let listener = TcpListener::bind(format!("0.0.0.0:{}", port)).unwrap();
    println!("VexFS Daemon running on http://localhost:{}", port);
    println!("Monitoring mountpoint: {}", mountpoint.display());
    println!("Serving dashboard from: {}", dashboard_dir.display());

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let mnt = mountpoint.clone();
                let dash = dashboard_dir.clone();
                thread::spawn(move || {
                    handle_client(stream, mnt, dash);
                });
            }
            Err(e) => {
                eprintln!("Error fixing connection: {}", e);
            }
        }
    }
}
