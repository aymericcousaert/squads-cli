#![allow(deprecated)]

use assert_cmd::Command;
use predicates::prelude::*;

/// Test that the CLI shows help
#[test]
fn test_help() {
    let mut cmd = Command::cargo_bin("squads-cli").unwrap();
    cmd.arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Microsoft Teams CLI"));
}

/// Test that version flag works
#[test]
fn test_version() {
    let mut cmd = Command::cargo_bin("squads-cli").unwrap();
    cmd.arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("squads-cli"));
}

/// Test that unknown commands fail gracefully
#[test]
fn test_unknown_command() {
    let mut cmd = Command::cargo_bin("squads-cli").unwrap();
    cmd.arg("unknown-command").assert().failure();
}

/// Test auth subcommand help
#[test]
fn test_auth_help() {
    let mut cmd = Command::cargo_bin("squads-cli").unwrap();
    cmd.args(["auth", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Authentication commands"));
}

/// Test chats subcommand help
#[test]
fn test_chats_help() {
    let mut cmd = Command::cargo_bin("squads-cli").unwrap();
    cmd.args(["chats", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Chat operations"));
}

/// Test mail subcommand help
#[test]
fn test_mail_help() {
    let mut cmd = Command::cargo_bin("squads-cli").unwrap();
    cmd.args(["mail", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Outlook mail operations"));
}

/// Test calendar subcommand help
#[test]
fn test_calendar_help() {
    let mut cmd = Command::cargo_bin("squads-cli").unwrap();
    cmd.args(["calendar", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Outlook calendar operations"));
}

/// Test teams subcommand help
#[test]
fn test_teams_help() {
    let mut cmd = Command::cargo_bin("squads-cli").unwrap();
    cmd.args(["teams", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Teams operations"));
}

/// Test teams reply needs content or --stdin, but not both
#[test]
fn test_teams_reply_content_rules() {
    let base = ["teams", "reply", "t", "c", "--message-id", "m"];
    let mut cmd = Command::cargo_bin("squads-cli").unwrap();
    cmd.args(base).assert().code(2);
    let mut cmd = Command::cargo_bin("squads-cli").unwrap();
    cmd.args(base).args(["hi", "--stdin"]).assert().code(2);
    let mut cmd = Command::cargo_bin("squads-cli").unwrap();
    cmd.args(base)
        .args(["hi", "--html", "--markdown"])
        .assert()
        .code(2);
}

/// Test teams post needs content or --stdin, and --html conflicts with --markdown
#[test]
fn test_teams_post_content_rules() {
    let base = ["teams", "post", "t", "c"];
    let mut cmd = Command::cargo_bin("squads-cli").unwrap();
    cmd.args(base).assert().code(2);
    let mut cmd = Command::cargo_bin("squads-cli").unwrap();
    cmd.args(base)
        .args(["hi", "--html", "--markdown"])
        .assert()
        .code(2);
}

/// Test sheets subcommand help
#[test]
fn test_sheets_help() {
    let mut cmd = Command::cargo_bin("squads-cli").unwrap();
    cmd.args(["sheets", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("SharePoint"));
}

/// Test format flag validation
#[test]
fn test_format_json() {
    let mut cmd = Command::cargo_bin("squads-cli").unwrap();
    cmd.args(["-f", "json", "--help"]).assert().success();
}

/// Test invalid format flag
#[test]
fn test_invalid_format() {
    let mut cmd = Command::cargo_bin("squads-cli").unwrap();
    cmd.args(["-f", "invalid", "chats", "list"])
        .assert()
        .failure();
}
