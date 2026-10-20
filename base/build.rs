// At build time, we grab the release version from the git tag.
// This is used by INFO("release")
use std::path::Path;
use std::process::Command;

fn run_git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Reruns the script when HEAD moves, its branch gets a commit, or a tag
/// changes. Paths come from git because this crate is not the repository
/// root and a worktree keeps HEAD apart from the shared refs; a path that
/// does not exist would make cargo rerun the script on every build.
fn watch_git() {
    let git_dir = run_git(&["rev-parse", "--absolute-git-dir"]);
    let common = run_git(&["rev-parse", "--path-format=absolute", "--git-common-dir"]);
    let (Some(git_dir), Some(common)) = (git_dir, common) else {
        println!("cargo:rerun-if-changed=build.rs");
        return;
    };
    let mut paths = vec![format!("{git_dir}/HEAD")];
    if let Some(branch) = run_git(&["symbolic-ref", "-q", "HEAD"]) {
        paths.push(format!("{common}/{branch}"));
    }
    paths.push(format!("{common}/packed-refs"));
    paths.push(format!("{common}/refs/tags"));
    for path in paths.iter().filter(|p| Path::new(p).exists()) {
        println!("cargo:rerun-if-changed={path}");
    }
}

fn main() {
    watch_git();

    // Prefer exact tag
    let version = run_git(&["describe", "--tags", "--exact-match"])
        .or_else(|| run_git(&["describe", "--tags", "--dirty", "--always"]))
        .unwrap_or_else(|| "unknown".into());

    println!("cargo:rustc-env=GIT_VERSION={}", version);
}
