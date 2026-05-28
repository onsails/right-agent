use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let manifest_dir =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set"));
    let frontend = manifest_dir.join("frontend");
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR not set"));
    let dashboard_out = out_dir.join("dashboard");

    // Tell cargo which sources to watch
    for path in [
        "frontend/src",
        "frontend/index.html",
        "frontend/vite.config.ts",
        "frontend/tsconfig.json",
        "frontend/package.json",
        "frontend/package-lock.json",
        "build.rs",
    ] {
        println!("cargo:rerun-if-changed={path}");
    }

    require_tool("node");
    require_tool("npm");

    let mut npm = Command::new("npm");
    npm.args(["install"]).current_dir(&frontend);
    run(&mut npm);

    let mut vite = Command::new("npx");
    vite.args(["vite", "build"])
        .current_dir(&frontend)
        .env("VITE_OUT_DIR", &dashboard_out);
    run(&mut vite);

    let index = dashboard_out.join("index.html");
    assert!(
        index.exists(),
        "vite build completed but {} not found",
        index.display(),
    );
}

fn require_tool(name: &str) {
    if which(name).is_none() {
        eprintln!(
            "error: '{name}' not found on PATH. Enter the devenv shell ('devenv shell') or install Node.js (>= 20).",
        );
        std::process::exit(1);
    }
}

fn which(name: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    for dir in env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn run(cmd: &mut Command) {
    let program = cmd.get_program().to_owned();
    let status = cmd
        .status()
        .unwrap_or_else(|e| panic!("failed to spawn {program:?}: {e}"));
    assert!(status.success(), "{program:?} exited with {status}",);
}
