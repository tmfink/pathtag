#[cfg(feature = "_git-version")]
fn git(args: &[&str]) -> String {
    use std::process::Command;
    let output = Command::new("git")
        .args(args)
        .output()
        .expect("Failed to execute git");

    if !output.status.success() {
        panic!("Git command failed: {:?}", args);
    }

    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn main() {
    #[cfg(feature = "_git-version")]
    {
        use std::process::Command;

        let commit = git(&["rev-parse", "--short", "HEAD"]);
        let commit_date = git(&["log", "-1", "--format=%cs"]);

        let dirty = !Command::new("git")
            .args(["diff", "--quiet", "HEAD", "--"])
            .status()
            .expect("Failed to check git status")
            .success();

        let version = format!(
            "{commit}{} {commit_date}",
            if dirty { "-dirty" } else { "" }
        );

        println!("cargo:rustc-env=GIT_VERSION={}", version);

        println!("cargo:rerun-if-changed=.git/HEAD");
        println!("cargo:rerun-if-changed=.git/index");
    }
}
