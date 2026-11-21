use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

const TIMEOUT: Duration = Duration::from_secs(15);

struct Host(Child);

impl Drop for Host {
    fn drop(&mut self) {
        // Closing stdin also lets the managed host exit if the supervisor is killed.
        drop(self.0.stdin.take());
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn lines(reader: impl Read + Send + 'static) -> Receiver<String> {
    let (send, receive) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(reader).lines().map_while(Result::ok) {
            if send.send(line).is_err() {
                break;
            }
        }
    });
    receive
}

fn wait_for(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + TIMEOUT;
    while !ready() {
        assert!(Instant::now() < deadline, "hot reload did not advance");
        thread::sleep(Duration::from_millis(30));
    }
}

fn generation(directory: &Path) -> String {
    fs::read_to_string(directory.join("current")).unwrap()
}

#[test]
fn dev_replaces_code_without_restarting_the_host_and_recovers_after_errors() {
    let folder = tempfile::tempdir().unwrap();
    let project = folder.path();
    fs::write(
        project.join("reload.toml"),
        r#"[package]
name = "Tests.ReloadView"
kind = "library"
sources = ["view.oxel"]
"#,
    )
    .unwrap();
    fs::write(
        project.join("oxel.toml"),
        r#"[package]
name = "reload_host"
framework = true
sources = ["host.oxel", "loader.oxel"]
"#,
    )
    .unwrap();
    fs::write(
        project.join("loader.oxel"),
        include_str!("../../../examples/hot_reload/loader.oxel"),
    )
    .unwrap();
    fs::write(
        project.join("host.oxel"),
        r#"
use System::{Console, Environment, Exception, Func};

fn main(args: string[]) -> i32 {
    assert(args[0] == "forwarded");
    let directory = Environment.GetEnvironmentVariable("OXEL_RELOAD_DIRECTORY") ?? throw new Exception("Missing reload directory");
    Console.WriteLine(directory);
    let mut seen = reload_generation(directory);
    let mut view = load_view::<Func<i32, i32>>(directory, seen, "Tests.ReloadView.Program", "update");
    let mut count = 0;
    while let Some(line) = Console.ReadLine() {
        let generation = try_reload_generation(directory) ?? seen;
        if generation != seen {
            seen = generation;
            try {
                let candidate = load_view::<Func<i32, i32>>(directory, generation, "Tests.ReloadView.Program", "update");
                view.context.Unload();
                view = candidate;
            } catch (error: Exception) {
                Console.WriteLine("rejected");
            }
        }
        count = (view.invoke)(count);
        Console.WriteLine(count);
    }
    view.context.Unload();
    23
}
"#,
    )
    .unwrap();
    let view = project.join("view.oxel");
    let change = |body: &str| fs::write(&view, body).unwrap();
    change("fn update(count: i32) -> i32 { count + 1 }");

    let mut host = Host(
        Command::new(env!("CARGO_BIN_EXE_oxel"))
            .current_dir(project)
            .args(["dev", "--reload-manifest", "reload.toml", "--", "forwarded"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let output = lines(host.0.stdout.take().unwrap());
    let errors = lines(host.0.stderr.take().unwrap());
    let read = || {
        output
            .recv_timeout(TIMEOUT)
            .unwrap_or_else(|err| panic!("{err}: {:?}", errors.try_iter().collect::<Vec<_>>()))
    };
    let directory = PathBuf::from(read());
    assert_eq!(generation(&directory), "0");

    let mut stdin = host.0.stdin.take().unwrap();
    let mut step = || writeln!(stdin, "step").unwrap();
    step();
    assert_eq!(read(), "1");

    change("fn update(count: i32) -> i32 { count + 10 }");
    wait_for(|| generation(&directory) == "1");
    step();
    assert_eq!(read(), "11");

    change("fn broken(");
    let diagnostic = errors.recv_timeout(TIMEOUT).unwrap();
    assert!(diagnostic.contains("view.oxel") && diagnostic.contains("error[E01"));
    assert_eq!(generation(&directory), "1");
    step();
    assert_eq!(read(), "21");

    change("fn different() {}");
    wait_for(|| generation(&directory) == "2");
    step();
    assert_eq!(read(), "rejected");
    assert_eq!(read(), "31");

    change("fn update(count: i32) -> i32 { count * 2 }");
    wait_for(|| generation(&directory) == "3");
    assert!(!directory.join("0.dll").exists());
    step();
    assert_eq!(read(), "62");

    drop(stdin);
    wait_for(|| host.0.try_wait().unwrap().is_some());
    assert_eq!(host.0.wait().unwrap().code(), Some(23));
    assert!(!directory.exists());
}
