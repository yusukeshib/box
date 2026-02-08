mod docker;
mod git;
mod session;

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use std::fs;
use std::path::Path;

#[derive(Parser)]
#[command(name = "realm", about = "Sandboxed Docker environments for git repos")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Create, resume, or list sessions
    Switch {
        /// Create a new session
        #[arg(short = 'c')]
        create: bool,

        /// Session name
        name: Option<String>,

        /// Docker image to use (default: alpine/git)
        #[arg(long, conflicts_with = "dockerfile")]
        image: Option<String>,

        /// Build image from Dockerfile (or set REALM_DOCKERFILE)
        #[arg(long, env = "REALM_DOCKERFILE", conflicts_with = "image")]
        dockerfile: Option<String>,

        /// Mount path inside container (default: /workspace)
        #[arg(long = "mount")]
        mount_path: Option<String>,

        /// Project directory (default: current directory)
        #[arg(short = 'd', long = "dir")]
        dir: Option<String>,

        /// Command to run in container
        #[arg(last = true)]
        cmd: Vec<String>,
    },
    /// Delete a session
    Remove {
        /// Session name
        name: String,
    },
}

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Some(Commands::Switch {
            create,
            name,
            image,
            dockerfile,
            mount_path,
            dir,
            cmd,
        }) => {
            if create {
                cmd_switch_create(name, image, dockerfile, mount_path, dir, cmd)
            } else if let Some(name) = name {
                cmd_switch_resume(&name, cmd)
            } else {
                cmd_list()
            }
        }
        Some(Commands::Remove { name }) => cmd_remove(&name),
        None => {
            Cli::parse_from(["realm", "--help"]);
            Ok(())
        }
    };

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

fn cmd_switch_create(
    name: Option<String>,
    image: Option<String>,
    dockerfile: Option<String>,
    mount_path: Option<String>,
    dir: Option<String>,
    cmd: Vec<String>,
) -> Result<()> {
    let name = match name {
        Some(n) => n,
        None => {
            bail!("Session name is required.\nUsage: realm switch -c <name> [options] [-- cmd...]")
        }
    };
    session::validate_name(&name)?;

    let project_dir = match dir {
        Some(d) => fs::canonicalize(&d)
            .map_err(|_| anyhow::anyhow!("Directory '{}' not found.", d))?
            .to_string_lossy()
            .to_string(),
        None => fs::canonicalize(".")
            .map_err(|_| anyhow::anyhow!("Cannot resolve current directory."))?
            .to_string_lossy()
            .to_string(),
    };

    if !git::is_repo(Path::new(&project_dir)) {
        bail!("'{}' is not a git repository.", project_dir);
    }

    if session::session_exists(&name) {
        bail!(
            "Session '{}' already exists. Remove it first: realm remove {}",
            name,
            name
        );
    }

    docker::check()?;

    let mut final_image = image.unwrap_or_else(|| session::DEFAULT_IMAGE.to_string());
    let mut final_dockerfile: Option<String> = None;

    if let Some(df) = dockerfile {
        let canonical = fs::canonicalize(&df)
            .map_err(|_| anyhow::anyhow!("Dockerfile '{}' not found.", df))?
            .to_string_lossy()
            .to_string();
        final_image = docker::build_image(&name, &canonical)?;
        final_dockerfile = Some(canonical);
    }

    let mount = mount_path.unwrap_or_else(|| session::DEFAULT_MOUNT.to_string());

    let sess = session::Session {
        name: name.clone(),
        project_dir: project_dir.clone(),
        image: final_image.clone(),
        mount_path: mount.clone(),
        dockerfile: final_dockerfile,
        command: cmd.clone(),
    };
    session::save(&sess)?;

    let exit_code = docker::run_container(&name, &project_dir, &final_image, &mount, &cmd)?;
    git::reset_index(&project_dir);
    std::process::exit(exit_code);
}

fn cmd_switch_resume(name: &str, cmd: Vec<String>) -> Result<()> {
    session::validate_name(name)?;

    let sess = session::load(name)?;

    if !Path::new(&sess.project_dir).is_dir() {
        bail!("Project directory '{}' no longer exists.", sess.project_dir);
    }

    docker::check()?;

    let mut image = sess.image.clone();
    if let Some(ref df) = sess.dockerfile {
        if Path::new(df).exists() {
            image = docker::build_image(name, df)?;
        }
    }

    println!("Resuming session '{}'...", name);
    session::touch_resumed_at(name)?;

    let final_cmd = if cmd.is_empty() {
        sess.command.clone()
    } else {
        cmd
    };

    let exit_code = docker::run_container(
        name,
        &sess.project_dir,
        &image,
        &sess.mount_path,
        &final_cmd,
    )?;
    git::reset_index(&sess.project_dir);
    std::process::exit(exit_code);
}

fn cmd_list() -> Result<()> {
    let sessions = session::list()?;
    session::print_table(&sessions);
    Ok(())
}

fn cmd_remove(name: &str) -> Result<()> {
    session::validate_name(name)?;

    if !session::session_exists(name) {
        bail!("Session '{}' not found.", name);
    }

    session::remove_dir(name)?;
    println!("Session '{}' removed.", name);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use std::sync::Mutex;

    // Serialize tests that touch REALM_DOCKERFILE env var
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Helper to parse CLI args and extract the Switch variant fields.
    /// Temporarily clears REALM_DOCKERFILE to avoid conflicts in tests.
    fn parse_switch(
        args: &[&str],
    ) -> (
        bool,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Vec<String>,
    ) {
        let _lock = ENV_LOCK.lock().unwrap();
        let old_val = std::env::var("REALM_DOCKERFILE").ok();
        std::env::remove_var("REALM_DOCKERFILE");

        let mut full_args = vec!["realm", "switch"];
        full_args.extend_from_slice(args);
        let cli = Cli::try_parse_from(full_args).unwrap();

        if let Some(v) = old_val {
            std::env::set_var("REALM_DOCKERFILE", v);
        }

        match cli.command.unwrap() {
            Commands::Switch {
                create,
                name,
                image,
                dockerfile,
                mount_path,
                dir,
                cmd,
            } => (create, name, image, dockerfile, mount_path, dir, cmd),
            _ => panic!("Expected Switch command"),
        }
    }

    #[test]
    fn test_parse_switch_list() {
        let cli = Cli::try_parse_from(["realm", "switch"]).unwrap();
        match cli.command.unwrap() {
            Commands::Switch { create, name, .. } => {
                assert!(!create);
                assert!(name.is_none());
            }
            _ => panic!("Expected Switch command"),
        }
    }

    #[test]
    fn test_parse_switch_resume() {
        let (create, name, ..) = parse_switch(&["my-session"]);
        assert!(!create);
        assert_eq!(name.as_deref(), Some("my-session"));
    }

    #[test]
    fn test_parse_switch_create_flag_before_name() {
        let (create, name, ..) = parse_switch(&["-c", "my-session"]);
        assert!(create);
        assert_eq!(name.as_deref(), Some("my-session"));
    }

    #[test]
    fn test_parse_switch_create_flag_after_name() {
        // This is the bug fix — -c works after the name
        let (create, name, ..) = parse_switch(&["my-session", "-c"]);
        assert!(create);
        assert_eq!(name.as_deref(), Some("my-session"));
    }

    #[test]
    fn test_parse_switch_create_with_image() {
        let (create, name, image, ..) =
            parse_switch(&["-c", "my-session", "--image", "ubuntu:latest"]);
        assert!(create);
        assert_eq!(name.as_deref(), Some("my-session"));
        assert_eq!(image.as_deref(), Some("ubuntu:latest"));
    }

    #[test]
    fn test_parse_switch_create_with_mount() {
        let (create, name, _, _, mount_path, ..) =
            parse_switch(&["-c", "my-session", "--mount", "/src"]);
        assert!(create);
        assert_eq!(name.as_deref(), Some("my-session"));
        assert_eq!(mount_path.as_deref(), Some("/src"));
    }

    #[test]
    fn test_parse_switch_create_with_dir() {
        let (create, name, _, _, _, dir, _) =
            parse_switch(&["-c", "my-session", "-d", "/tmp/project"]);
        assert!(create);
        assert_eq!(name.as_deref(), Some("my-session"));
        assert_eq!(dir.as_deref(), Some("/tmp/project"));
    }

    #[test]
    fn test_parse_switch_with_command() {
        let (_, name, _, _, _, _, cmd) =
            parse_switch(&["my-session", "--", "bash", "-c", "echo hi"]);
        assert_eq!(name.as_deref(), Some("my-session"));
        assert_eq!(cmd, vec!["bash", "-c", "echo hi"]);
    }

    #[test]
    fn test_parse_switch_create_with_command() {
        let (create, name, _, _, _, _, cmd) = parse_switch(&["-c", "my-session", "--", "bash"]);
        assert!(create);
        assert_eq!(name.as_deref(), Some("my-session"));
        assert_eq!(cmd, vec!["bash"]);
    }

    #[test]
    fn test_parse_switch_create_all_options() {
        let (create, name, image, _, mount_path, dir, cmd) = parse_switch(&[
            "-c",
            "full-session",
            "--image",
            "python:3.11",
            "--mount",
            "/app",
            "-d",
            "/tmp/project",
            "--",
            "python",
            "main.py",
        ]);
        assert!(create);
        assert_eq!(name.as_deref(), Some("full-session"));
        assert_eq!(image.as_deref(), Some("python:3.11"));
        assert_eq!(mount_path.as_deref(), Some("/app"));
        assert_eq!(dir.as_deref(), Some("/tmp/project"));
        assert_eq!(cmd, vec!["python", "main.py"]);
    }

    #[test]
    fn test_parse_switch_image_dockerfile_conflict() {
        let full_args = vec![
            "realm",
            "switch",
            "-c",
            "test",
            "--image",
            "foo",
            "--dockerfile",
            "bar",
        ];
        let result = Cli::try_parse_from(full_args);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_remove() {
        let cli = Cli::try_parse_from(["realm", "remove", "my-session"]).unwrap();
        match cli.command.unwrap() {
            Commands::Remove { name } => {
                assert_eq!(name, "my-session");
            }
            _ => panic!("Expected Remove command"),
        }
    }

    #[test]
    fn test_parse_remove_missing_name() {
        let result = Cli::try_parse_from(["realm", "remove"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_no_command() {
        let cli = Cli::try_parse_from(["realm"]).unwrap();
        assert!(cli.command.is_none());
    }

    #[test]
    fn test_parse_switch_create_flag_at_end_with_options() {
        // Another variation: -c at the very end
        let (create, name, image, ..) =
            parse_switch(&["test-session", "--image", "ubuntu:latest", "-c"]);
        assert!(create);
        assert_eq!(name.as_deref(), Some("test-session"));
        assert_eq!(image.as_deref(), Some("ubuntu:latest"));
    }

    #[test]
    fn test_parse_switch_dir_long_form() {
        let (_, _, _, _, _, dir, _) = parse_switch(&["-c", "sess", "--dir", "/tmp/project"]);
        assert_eq!(dir.as_deref(), Some("/tmp/project"));
    }

    #[test]
    fn test_parse_switch_empty_command_after_separator() {
        let (_, name, _, _, _, _, cmd) = parse_switch(&["my-session", "--"]);
        assert_eq!(name.as_deref(), Some("my-session"));
        assert!(cmd.is_empty());
    }
}
