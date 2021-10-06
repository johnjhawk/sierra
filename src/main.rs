use chrono::prelude::*;
use chrono::Duration;
use clap::{load_yaml, App};
use git2::BranchType;
use git2::{Oid, Repository};
use std::collections::HashSet;
use std::convert::TryFrom;
use std::io;
use std::io::{Bytes, Read, Stdin, Stdout, Write};
use std::string::FromUtf8Error;
use termcolor::{Color, ColorChoice, ColorSpec, StandardStream, WriteColor};

/// Custom error type alias.
type Result<T, E = Error> = std::result::Result<T, E>;

/// Represent a git branch with commit attributes.
struct Branch<'repo> {
    /// Commit id.
    id: Oid,
    name: String,
    /// Author of the last commit to the branch.
    commit_author: String,

    /// Commit message of the last commit to the branch.
    commit_summary: String,
    branch_type: BranchType,
    // branch_type: &'repo str,
    commit_time: NaiveDateTime,
    is_head: bool,
    branch: git2::Branch<'repo>,
}

impl<'repo> Branch<'repo> {
    // Result<()> is short for Result<(), Error>
    fn delete(&mut self) -> Result<()> {
        self.branch.delete().map_err(From::from) // the fn says we return Error but delete return git2 error
                                                 // same as Ok(self.branch.delete()?)
    }
}

/// Custom _umbrella_ error type for main to return.
#[derive(Debug, thiserror::Error)]
enum Error {
    #[error(transparent)]
    CrosstermError(#[from] crossterm::ErrorKind),

    #[error(transparent)]
    IoError(#[from] io::Error),

    #[error(transparent)]
    GitError(#[from] git2::Error),

    #[error(transparent)]
    FromUtf8Error(#[from] FromUtf8Error),

    #[error("Invalid input, dont know '{0}'")]
    InvalidInput(char),
}

/// Possible user actions.
enum BranchAction {
    Keep,
    Delete,
    Quit,
}

impl TryFrom<char> for BranchAction {
    type Error = Error;

    fn try_from(value: char) -> Result<Self, Self::Error> {
        match value {
            'k' => Ok(BranchAction::Keep),
            'd' => Ok(BranchAction::Delete),
            'q' => Ok(BranchAction::Quit),
            _ => Err(Error::InvalidInput(value)),
        }
    }
}

/// Switch terminal color.
fn set_color(color: Color) -> io::Result<()> {
    let mut stdout = StandardStream::stdout(ColorChoice::Always);
    stdout.set_color(ColorSpec::new().set_fg(Some(color)))?;
    Ok(())
}

/// Prepare user input and give user feedback.
fn process_user_request(
    byte: u8,
    stdout: &mut Stdout,
    stdin: &mut Bytes<Stdin>,
    branch: &Branch,
) -> Result<BranchAction> {
    let c = char::from(byte);
    write!(stdout, "{}\r\n", c)?;

    if c == '?' {
        write!(stdout, "select from the following:\r\n")?;
        write!(stdout, "\tk - Keep the branch\r\n")?;
        write!(stdout, "\td - Delete the branch\r\n")?;
        write!(stdout, "\tq - Quit\r\n")?;
        write!(stdout, "\t? - Help\r\n")?;
        stdout.flush()?;
        request_user_input(stdout, stdin, branch)
    } else {
        BranchAction::try_from(c)
    }
}

/// Ask for import from user.
fn request_user_input(
    stdout: &mut Stdout,
    stdin: &mut Bytes<Stdin>,
    branch: &Branch,
) -> Result<BranchAction> {
    match branch.branch_type {
        BranchType::Local => set_color(Color::Green)?,
        BranchType::Remote => set_color(Color::Cyan)?,
    }

    write!(
        stdout,
        "\n\r'{}' ({})",
        branch_type_to_str(branch.branch_type),
        branch.name,
    )?;

    write!(
        stdout,
        "\n\r\tlast commit as {}\n\r\tlast commit id: {} \n\r\tcommit author: {} \n\r\tcommit summary: {}\n\r",
        branch.commit_time,
        &branch.id.to_string()[1..=10],
        branch.commit_author,
        branch.commit_summary,
    )?;

    set_color(Color::Blue)?;
    write!(stdout, "(k/d/q/?) > ",)?;

    stdout.flush()?;

    let byte = match stdin.next() {
        Some(byte) => byte?,
        None => return request_user_input(stdout, stdin, branch), // todo check with ctrl d
    };

    process_user_request(byte, stdout, stdin, branch)
}

/// Transform enum instance to string slice.
fn branch_type_to_str(branch_type: git2::BranchType) -> &'static str {
    match branch_type {
        BranchType::Local => "local",
        BranchType::Remote => "remote",
    }
}

/// Interact with git branches.
fn get_branches<'a>(
    repo: &'a Repository,
    ignore: &HashSet<String>,
    filter_in: Option<&str>,
    local_only: &bool,
) -> Result<Vec<Branch<'a>>> {

    let local_only = local_only.then(|| BranchType::Local);

    let mut branches = repo
        .branches(local_only)?
        .map(|branch| {
            let (branch, branch_type) = branch?;
            let name = String::from_utf8(branch.name_bytes()?.to_vec())?;
            let commit = branch.get().peel_to_commit()?;

            let commit_time = commit.time();
            let commit_author = commit.author().name().unwrap_or("no author").to_owned();
            let commit_summary = commit.summary().unwrap_or("no summary").to_owned();
            let offset = Duration::minutes(i64::from(commit_time.offset_minutes()));

            let commit_time = NaiveDateTime::from_timestamp(commit_time.seconds(), 0) + offset;
            Ok(Branch {
                id: commit.id(),
                commit_author,
                commit_summary,
                commit_time,
                branch_type,
                name,
                is_head: branch.is_head(),
                branch,
            })
        })
        .filter(|branch| {
            if let Ok(branch) = branch {
                if filter_in.is_some() {
                    let fo: &str = &*filter_in.unwrap().to_lowercase(); //convert String to &str

                    let bn_lower: &str = &*branch.name.to_lowercase();

                    !ignore.contains(&branch.name) && bn_lower.contains(fo)
                } else {
                    !ignore.contains(&branch.name)
                }
            } else {
                true
            }
        })
        .collect::<Result<Vec<_>>>()?;

    branches.sort_unstable_by_key(|branch| branch.commit_time);

    Ok(branches)
}

fn main() {
    let yaml = load_yaml!("../cli.yaml");
    let matches = App::from(yaml).get_matches();

    let filter_in = matches.value_of("filter_in");
    let local_only = matches.is_present("local_only");

    let indelible_branches: HashSet<String> = vec![
        String::from("origin/main"),
        String::from("main"),
        String::from("origin/master"),
        String::from("master"),
        String::from("origin/default"),
        String::from("origin/HEAD"),
        String::from("default"),
    ]
    .into_iter()
    .collect();

    let result = (|| -> Result<_> {
        let repo = Repository::open_from_env()?;

        crossterm::terminal::enable_raw_mode()?;

        let mut stdout = io::stdout();
        let mut stdin = io::stdin().bytes();

        let branches = &mut get_branches(&repo, &indelible_branches, filter_in, &local_only)?;

        if branches.is_empty() {
            set_color(Color::Yellow)?;
            write!(
                stdout,
                "No branches found. Ignoring: {:?}\r\n",
                indelible_branches
            )?;
        } else {
            set_color(Color::Yellow)?;
            write!(stdout, "{:?} Total Branches Found (", branches.len(),)?;
            write!(
                stdout,
                "{:?} Local and ",
                branches
                    .iter()
                    .filter(|b| b.branch_type == BranchType::Local)
                    .count(),
            )?;
            write!(
                stdout,
                "{:?} Remote)\n\r",
                branches
                    .iter()
                    .filter(|b| b.branch_type == BranchType::Remote)
                    .count(),
            )?;

            for branch in branches {
                //write!(stdout, "author: {}", branch.author).expect("no author");
                //write!(stdout, "author: {}", branch.author).unwrap();
                //write!(stdout, "author: {}", branch.commit_author)?;
                //write!(stdout, "summary: {}", branch.commit_summary)?;

                if branch.is_head {
                    set_color(Color::Yellow)?;
                    write!(stdout, "Ignoring current branch: '{}'\r\n", branch.name)?
                } else {
                    match request_user_input(&mut stdout, &mut stdin, &branch)? {
                        BranchAction::Quit => return Ok(()),
                        BranchAction::Keep => {
                            write!(stdout, "")?;
                        }
                        BranchAction::Delete => {
                            if branch.branch_type == BranchType::Local {
                                branch.delete()?;
                                set_color(Color::Red)?;
                                write!(stdout, "'{}' was deleted.\r\n ", branch.name)?;

                                set_color(Color::White)?;
                                write!(
                                    stdout,
                                    "to undo, run:\r\n \tgit branch {} {}\r\n\n",
                                    branch.name, branch.id
                                )?
                            } else {
                                set_color(Color::Red)?;
                                write!(stdout, "\tI don't want to be responsible for deleting remote branches. \n\r\tgithub.com has a great interface for such endeavours.\r\n")?;
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    })();

    // ok() converts self into an Option<T>, consuming self, and discarding the error, if any.
    crossterm::terminal::disable_raw_mode().ok();

    match result {
        Ok(()) => {}
        Err(error) => {
            eprintln!("{}", error);
            std::process::exit(1);
        }
    }
}

mod branch_tests {

    #[test]
    fn test_branch_type_to_str_a() {
        let result = crate::branch_type_to_str(git2::BranchType::Local);
        assert!("remote" != result);
        assert!("local" == result);
    }

    #[test]
    fn test_branch_type_to_str_b() {
        let result = crate::branch_type_to_str(git2::BranchType::Remote);
        assert!("remote" == result);
        assert!("local" != result);
    }
}
