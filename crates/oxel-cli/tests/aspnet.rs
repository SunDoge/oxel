use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpStream},
    path::Path,
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};

fn oxel(project: &Path, action: &str) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_oxel"))
        .current_dir(project)
        .arg(action)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{action}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn request(address: SocketAddr, method: &str, path: &str, payload: &str) -> (u16, String) {
    let timeout = Duration::from_secs(5);
    let mut socket = TcpStream::connect_timeout(&address, timeout).unwrap();
    socket.set_read_timeout(Some(timeout)).unwrap();
    socket.set_write_timeout(Some(timeout)).unwrap();
    write!(
        socket,
        "{method} {path} HTTP/1.0\r\nHost: {address}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{payload}",
        payload.len()
    )
    .unwrap();
    let mut response = String::new();
    socket.read_to_string(&mut response).unwrap();
    let (headers, body) = response.split_once("\r\n\r\n").unwrap();
    let status = headers.split_whitespace().nth(1).unwrap().parse().unwrap();
    if status == 200 {
        assert!(
            headers
                .to_ascii_lowercase()
                .contains("content-type: application/json")
        );
    }
    (status, body.to_owned())
}

#[test]
fn aspnet_minimal_api_serves_async_routes_and_binds_json() {
    let folder = tempfile::tempdir().unwrap();
    let project = folder.path();
    for (name, content) in [
        (
            "oxel.toml",
            include_str!("../../../examples/aspnet_api/oxel.toml"),
        ),
        (
            "main.oxel",
            include_str!("../../../examples/aspnet_api/main.oxel"),
        ),
    ] {
        fs::write(project.join(name), content).unwrap();
    }
    oxel(project, "check");
    let artifact = project.join(oxel(project, "compile").trim());
    let runtime_config: serde_json::Value =
        serde_json::from_slice(&fs::read(artifact.with_extension("runtimeconfig.json")).unwrap())
            .unwrap();
    let frameworks = runtime_config["runtimeOptions"]["frameworks"]
        .as_array()
        .unwrap();
    for name in ["Microsoft.NETCore.App", "Microsoft.AspNetCore.App"] {
        assert!(frameworks.iter().any(|framework| framework["name"] == name));
    }
    assert!(!artifact.with_file_name("Microsoft.AspNetCore.dll").exists());

    let mut server = Server(
        Command::new("dotnet")
            .current_dir(project)
            .arg(artifact)
            .args([
                "--urls",
                "http://127.0.0.1:0",
                "--Logging:LogLevel:Microsoft.Hosting.Lifetime",
                "Information",
            ])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let stdout = server.0.stdout.take().unwrap();
    let (send, receive) = mpsc::channel();
    let logs = thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(address) = line.trim().strip_prefix("Now listening on: http://") {
                let _ = send.send(address.parse::<SocketAddr>().unwrap());
            }
        }
    });
    let address = receive
        .recv_timeout(Duration::from_secs(15))
        .expect("Kestrel did not report its bound address");
    for (method, path, payload, status, body) in [
        ("GET", "/health", "", 200, r#""ok""#),
        ("GET", "/add/10/20", "", 200, r#"{"sum":30}"#),
        ("GET", "/add/bad/20", "", 404, ""),
        ("POST", "/sum", "[10,20]", 200, r#"{"sum":30}"#),
        ("POST", "/sum", "[invalid]", 400, ""),
    ] {
        let response = request(address, method, path, payload);
        assert_eq!(response, (status, body.to_owned()), "{method} {path}");
    }
    drop(server);
    logs.join().unwrap();
}
