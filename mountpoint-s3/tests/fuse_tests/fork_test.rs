// These tests all run the main binary and so expect to be able to reach S3
#![cfg(feature = "s3_tests")]

use assert_cmd::prelude::*;
#[cfg(not(feature = "s3express_tests"))]
use aws_config::BehaviorVersion;
#[cfg(not(feature = "s3express_tests"))]
use aws_sdk_sts::config::Region;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, ExitStatus, Stdio};
use std::time::{Duration, Instant};
use std::{path::PathBuf, process::Command};
use test_case::test_case;

use crate::common::fuse::read_dir_to_entry_names;
use crate::common::s3::{create_objects, get_test_bucket_and_prefix, get_test_bucket_forbidden, get_test_region};
#[cfg(not(feature = "s3express_tests"))]
use crate::common::s3::{get_subsession_iam_role, tokio_block_on};

const MAX_WAIT_DURATION: std::time::Duration = std::time::Duration::from_secs(10);

#[test]
fn run_in_background() -> Result<(), Box<dyn std::error::Error>> {
    let (bucket, prefix) = get_test_bucket_and_prefix("test_run_in_background");
    let region = get_test_region();
    let mount_point = assert_fs::TempDir::new()?;

    let mut cmd = Command::cargo_bin("mount-s3")?;
    let child = cmd
        .arg(&bucket)
        .arg(mount_point.path())
        .arg(format!("--prefix={prefix}"))
        .arg("--auto-unmount")
        .arg(format!("--region={region}"))
        .spawn()
        .expect("unable to spawn child");

    let exit_status = wait_for_exit(child);

    // verify mount status and mount entry
    assert!(exit_status.success());
    assert!(mount_exists("mountpoint-s3", mount_point.path().to_str().unwrap()));

    test_read_files(&bucket, &prefix, &region, &mount_point.to_path_buf());

    unmount(mount_point.path());

    Ok(())
}

#[test]
fn run_in_background_region_from_env() -> Result<(), Box<dyn std::error::Error>> {
    let (bucket, prefix) = get_test_bucket_and_prefix("test_run_in_background_region_from_env");
    let region = get_test_region();
    let mount_point = assert_fs::TempDir::new()?;

    let mut cmd = Command::cargo_bin("mount-s3")?;
    let child = cmd
        .arg(&bucket)
        .arg(mount_point.path())
        .arg(format!("--prefix={prefix}"))
        .arg("--auto-unmount")
        .env("AWS_REGION", region.clone())
        .spawn()
        .expect("unable to spawn child");

    let exit_status = wait_for_exit(child);

    // verify mount status and mount entry
    assert!(exit_status.success());
    assert!(mount_exists("mountpoint-s3", mount_point.path().to_str().unwrap()));

    test_read_files(&bucket, &prefix, &region, &mount_point.to_path_buf());

    unmount(mount_point.path());

    Ok(())
}

#[test]
// Automatic region resolution doesn't work with S3 Express One Zone
#[cfg(not(feature = "s3express_tests"))]
fn run_in_background_automatic_region_resolution() -> Result<(), Box<dyn std::error::Error>> {
    let (bucket, prefix) = get_test_bucket_and_prefix("test_run_in_background_automatic_region_resolution");
    let region = get_test_region();
    let mount_point = assert_fs::TempDir::new()?;

    let mut cmd = Command::cargo_bin("mount-s3")?;
    let child = cmd
        .arg(&bucket)
        .arg(mount_point.path())
        .arg(format!("--prefix={prefix}"))
        .arg("--auto-unmount")
        .spawn()
        .expect("unable to spawn child");

    let exit_status = wait_for_exit(child);

    // verify mount status and mount entry
    assert!(exit_status.success());
    assert!(mount_exists("mountpoint-s3", mount_point.path().to_str().unwrap()));

    test_read_files(&bucket, &prefix, &region, &mount_point.to_path_buf());

    unmount(mount_point.path());

    Ok(())
}

#[test]
fn run_in_foreground() -> Result<(), Box<dyn std::error::Error>> {
    let (bucket, prefix) = get_test_bucket_and_prefix("test_run_in_foreground");
    let region = get_test_region();
    let mount_point = assert_fs::TempDir::new()?;

    let mut cmd = Command::cargo_bin("mount-s3")?;
    let mut child = cmd
        .arg(&bucket)
        .arg(mount_point.path())
        .arg(format!("--prefix={prefix}"))
        .arg("--auto-unmount")
        .arg("--foreground")
        .arg(format!("--region={region}"))
        .spawn()
        .expect("unable to spawn child");

    wait_for_mount("mountpoint-s3", mount_point.path().to_str().unwrap());

    // verify that process is still alive
    let child_status = child.try_wait().unwrap();
    assert_eq!(None, child_status);

    assert!(mount_exists("mountpoint-s3", mount_point.path().to_str().unwrap()));

    test_read_files(&bucket, &prefix, &region, &mount_point.to_path_buf());

    unmount(mount_point.path());

    Ok(())
}

#[test]
fn run_in_background_fail_on_mount() -> Result<(), Box<dyn std::error::Error>> {
    // the mount would fail from error 403 on HeadBucket
    let bucket = get_test_bucket_forbidden();
    let mount_point = assert_fs::TempDir::new()?;

    let mut cmd = Command::cargo_bin("mount-s3")?;
    let child = cmd
        .arg(&bucket)
        .arg(mount_point.path())
        .arg("--auto-unmount")
        .spawn()
        .expect("unable to spawn child");

    let exit_status = wait_for_exit(child);

    // verify mount status and mount entry
    assert!(!exit_status.success());
    assert!(!mount_exists("mountpoint-s3", mount_point.path().to_str().unwrap()));

    Ok(())
}

#[test]
fn run_in_foreground_fail_on_mount() -> Result<(), Box<dyn std::error::Error>> {
    // the mount would fail from error 403 on HeadBucket
    let bucket = get_test_bucket_forbidden();
    let mount_point = assert_fs::TempDir::new()?;

    let mut cmd = Command::cargo_bin("mount-s3")?;
    let child = cmd
        .arg(&bucket)
        .arg(mount_point.path())
        .arg("--auto-unmount")
        .arg("--foreground")
        .spawn()
        .expect("unable to spawn child");

    let exit_status = wait_for_exit(child);

    // verify mount status and mount entry
    assert!(!exit_status.success());
    assert!(!mount_exists("mountpoint-s3", mount_point.path().to_str().unwrap()));

    Ok(())
}

#[test]
fn run_fail_on_duplicate_mount() -> Result<(), Box<dyn std::error::Error>> {
    let (bucket, prefix) = get_test_bucket_and_prefix("run_fail_on_duplicate_mount");
    let mount_point = assert_fs::TempDir::new()?;
    let region = get_test_region();

    let mut cmd = Command::cargo_bin("mount-s3")?;
    let first_mount = cmd
        .arg(&bucket)
        .arg(mount_point.path())
        .arg(format!("--prefix={prefix}"))
        .arg("--auto-unmount")
        .arg(format!("--region={region}"))
        .spawn()
        .expect("unable to spawn child");

    let exit_status = wait_for_exit(first_mount);

    // verify mount status and mount entry
    assert!(exit_status.success());
    assert!(mount_exists("mountpoint-s3", mount_point.path().to_str().unwrap()));

    let mut cmd = Command::cargo_bin("mount-s3")?;
    let second_mount = cmd
        .arg(&bucket)
        .arg(mount_point.path())
        .arg(format!("--prefix={prefix}"))
        .arg("--auto-unmount")
        .spawn()
        .expect("unable to spawn child");

    let exit_status = wait_for_exit(second_mount);

    // verify mount status
    assert!(!exit_status.success());

    unmount(mount_point.path());

    Ok(())
}

#[test]
fn mount_readonly() -> Result<(), Box<dyn std::error::Error>> {
    let (bucket, prefix) = get_test_bucket_and_prefix("test_mount_readonly");
    let mount_point = assert_fs::TempDir::new()?;
    let region = get_test_region();

    let mut cmd = Command::cargo_bin("mount-s3")?;
    let mut child = cmd
        .arg(&bucket)
        .arg(mount_point.path())
        .arg(format!("--prefix={prefix}"))
        .arg("--auto-unmount")
        .arg("--foreground")
        .arg("--read-only")
        .arg(format!("--region={region}"))
        .spawn()
        .expect("unable to spawn child");

    wait_for_mount("mountpoint-s3", mount_point.path().to_str().unwrap());

    // verify that process is still alive
    let child_status = child.try_wait().unwrap();
    assert_eq!(None, child_status);

    let mount_line =
        get_mount_from_source_and_mountpoint("mountpoint-s3", mount_point.path().to_str().unwrap()).unwrap();

    // mount entry looks like
    // /dev/nvme0n1p2 on /boot type ext4 (rw,relatime)
    let mount_opts_str = mount_line.split_whitespace().last().unwrap();
    let mount_opts: Vec<&str> = mount_opts_str.trim_matches(&['(', ')'] as &[_]).split(',').collect();
    assert!(mount_opts.contains(&"ro"));

    unmount(mount_point.path());

    Ok(())
}

#[test_case(true)]
#[test_case(false)]
fn mount_allow_delete(allow_delete: bool) -> Result<(), Box<dyn std::error::Error>> {
    let (bucket, prefix) = get_test_bucket_and_prefix("mount_allow_delete");
    let mount_point = assert_fs::TempDir::new()?;
    let region = get_test_region();

    let mut cmd = Command::cargo_bin("mount-s3")?;
    cmd.arg(&bucket)
        .arg(mount_point.path())
        .arg(format!("--prefix={prefix}"))
        .arg("--auto-unmount")
        .arg(format!("--region={region}"));
    if allow_delete {
        cmd.arg("--allow-delete");
    }
    let child = cmd.spawn().expect("unable to spawn child");

    let exit_status = wait_for_exit(child);

    // verify mount status and mount entry
    assert!(exit_status.success());
    assert!(mount_exists("mountpoint-s3", mount_point.path().to_str().unwrap()));

    // create and try to delete an object
    create_objects(&bucket, &prefix, &region, "file.txt", b"hello world");

    let result = fs::remove_file(mount_point.path().join("file.txt"));
    if allow_delete {
        result.expect("remove file should succeed when --allow_delete is set");
    } else {
        result.expect_err("remove file should fail when --allow_delete is not set");
    }

    unmount(mount_point.path());

    Ok(())
}

#[test]
// S3 Express One Zone doesn't support scoped credentials
#[cfg(not(feature = "s3express_tests"))]
fn mount_scoped_credentials() -> Result<(), Box<dyn std::error::Error>> {
    let (bucket, prefix) = get_test_bucket_and_prefix("mount_allow_delete");
    let subprefix = format!("{prefix}sub/");
    let mount_point = assert_fs::TempDir::new()?;
    let region = get_test_region();
    let subsession_role = get_subsession_iam_role();

    // Get scoped down credentials to the subprefix
    let policy = r#"{"Statement": [
        {"Effect": "Allow", "Action": ["s3:GetObject", "s3:PutObject", "s3:DeleteObject", "s3:AbortMultipartUpload"], "Resource": "arn:aws:s3:::__BUCKET__/__PREFIX__*"},
        {"Effect": "Allow", "Action": "s3:ListBucket", "Resource": "arn:aws:s3:::__BUCKET__", "Condition": {"StringLike": {"s3:prefix": "__PREFIX__*"}}}
    ]}"#;
    let policy = policy.replace("__BUCKET__", &bucket).replace("__PREFIX__", &subprefix);
    let config = tokio_block_on(
        aws_config::defaults(BehaviorVersion::latest())
            .region(Region::new(get_test_region()))
            .load(),
    );
    let sts_client = aws_sdk_sts::Client::new(&config);
    let credentials = tokio_block_on(
        sts_client
            .assume_role()
            .role_arn(subsession_role)
            .role_session_name("test_scoped_credentials")
            .policy(policy)
            .send(),
    )
    .unwrap();
    let credentials = credentials.credentials().unwrap();

    // First try without the subprefix -- mount should fail as we don't have permissions on it
    let mut cmd = Command::cargo_bin("mount-s3")?;
    let child = cmd
        .arg(&bucket)
        .arg(mount_point.path())
        .arg(format!("--prefix={prefix}"))
        .arg("--auto-unmount")
        .arg(format!("--region={region}"))
        .env("AWS_ACCESS_KEY_ID", credentials.access_key_id())
        .env("AWS_SECRET_ACCESS_KEY", credentials.secret_access_key())
        .env("AWS_SESSION_TOKEN", credentials.session_token())
        .spawn()
        .expect("unable to spawn child");

    let exit_status = wait_for_exit(child);

    // verify mount status and mount entry
    assert!(!exit_status.success());
    assert!(!mount_exists("mountpoint-s3", mount_point.path().to_str().unwrap()));

    // Now try with the subprefix -- mount should work since we have the right permissions
    let mut cmd = Command::cargo_bin("mount-s3")?;
    let child = cmd
        .arg(&bucket)
        .arg(mount_point.path())
        .arg(format!("--prefix={subprefix}"))
        .arg("--auto-unmount")
        .arg(format!("--region={region}"))
        .env("AWS_ACCESS_KEY_ID", credentials.access_key_id())
        .env("AWS_SECRET_ACCESS_KEY", credentials.secret_access_key())
        .env("AWS_SESSION_TOKEN", credentials.session_token())
        .spawn()
        .expect("unable to spawn child");

    let exit_status = wait_for_exit(child);

    // verify mount status and mount entry
    assert!(exit_status.success());
    assert!(mount_exists("mountpoint-s3", mount_point.path().to_str().unwrap()));

    test_read_files(&bucket, &subprefix, &region, &mount_point.to_path_buf());

    unmount(mount_point.path());

    Ok(())
}

fn test_read_files(bucket: &str, prefix: &str, region: &str, mount_point: &PathBuf) {
    // create objects for test
    create_objects(bucket, prefix, region, "file1.txt", b"hello world");
    create_objects(bucket, prefix, region, "dir/file2.txt", b"hello world");

    // verify readdir works on mount point
    let read_dir_iter = fs::read_dir(mount_point).unwrap();
    let dir_entry_names = read_dir_to_entry_names(read_dir_iter);
    assert_eq!(dir_entry_names, vec!["dir", "file1.txt"]);

    // verify readdir works
    let read_dir_iter = fs::read_dir(mount_point.join("dir")).unwrap();
    let dir_entry_names = read_dir_to_entry_names(read_dir_iter);
    assert_eq!(dir_entry_names, vec!["file2.txt"]);

    // verify read file works
    let file_content = fs::read_to_string(mount_point.as_path().join("file1.txt")).unwrap();
    assert_eq!(file_content, "hello world");

    let file_content = fs::read_to_string(mount_point.as_path().join("dir/file2.txt")).unwrap();
    assert_eq!(file_content, "hello world");
}

fn mount_exists(source: &str, mount_point: &str) -> bool {
    get_mount_from_source_and_mountpoint(source, mount_point).is_some()
}

/// Read all mount records in the system and return the line that matches given arguments.
/// # Arguments
///
/// * `source` - name of the file system.
/// * `mount_point` - path to the mount point.
fn get_mount_from_source_and_mountpoint(source: &str, mount_point: &str) -> Option<String> {
    // macOS wrap its temp directory under /private but it's not visible to users
    #[cfg(target_os = "macos")]
    let mount_point = format!("/private{}", mount_point);

    let mut cmd = Command::new("mount");
    #[cfg(target_os = "linux")]
    cmd.arg("-l");
    let mut cmd = cmd.stdout(Stdio::piped()).spawn().expect("Unable to spawn mount tool");

    let stdout = cmd.stdout.as_mut().unwrap();
    let stdout_reader = BufReader::new(stdout);
    let stdout_lines = stdout_reader.lines();

    for line in stdout_lines.flatten() {
        let str: Vec<&str> = line.split_whitespace().collect();
        let source_rec = str[0];
        let mount_point_rec = str[2];
        if source_rec == source && mount_point_rec == mount_point {
            return Some(line);
        }
    }
    None
}

fn wait_for_exit(mut child: Child) -> ExitStatus {
    let st = Instant::now();

    loop {
        if st.elapsed() > MAX_WAIT_DURATION {
            panic!("wait for result timeout")
        }
        match child.try_wait().expect("unable to wait for result") {
            Some(result) => break result,
            None => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

fn wait_for_mount(source: &str, mount_point: &str) {
    let st = Instant::now();

    loop {
        if st.elapsed() > MAX_WAIT_DURATION {
            panic!("wait for mount timeout")
        }
        if mount_exists(source, mount_point) {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn unmount(mount_point: &Path) {
    fn run_fusermount(bin: &str, mount_point: &Path) -> Result<bool, Box<dyn std::error::Error>> {
        let mut child = Command::new(bin).arg("-u").arg(mount_point).spawn()?;
        let result = child.wait()?;
        Ok(result.success())
    }

    // Loop a bit to give any slow/async FUSE requests time to finish
    for i in 1..4 {
        // Try both FUSE 2 and FUSE 3 versions, since we don't know where we're running
        for bin in ["fusermount", "fusermount3"] {
            if matches!(run_fusermount(bin, mount_point), Ok(true)) {
                return;
            }
        }
        std::thread::sleep(i * Duration::from_secs(1));
    }

    panic!("failed to unmount");
}
