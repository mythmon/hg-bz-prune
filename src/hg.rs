//! Tools to interact with Mercurial.

use crate::bz::Bug;
use async_std::{
    io,
    path::PathBuf,
    process::{Command, Output, Stdio},
};
use serde::Deserialize;
use std::ffi::OsStr;
use thiserror::Error;

/// An error that prevented a mercurial command from being processed.
#[derive(Error, Debug)]
pub enum Error {
    /// Could not run command
    #[error("Could not run command")]
    Io(#[from] io::Error),

    /// Mercurial returned a non-zero status code
    #[error("Mercurial error {stdout}{stderr}")]
    Command {
        /// The stdout sent by Mercurial. This is usually empty.
        stdout: String,
        /// The stderr sent by Mercurial. This usually describes what went wrong.
        stderr: String,
    },

    /// Output was not valid UTF-8
    #[error("Output was not valid UTF-8")]
    Utf8(#[from] std::string::FromUtf8Error),

    /// Mercurial output could not be parsed
    #[error("Mercurial output could not be parsed")]
    RevisionParse(#[from] serde_json::error::Error),
}

type Result<T> = std::result::Result<T, Error>;

impl Error {
    fn command_error(output: &Output) -> Self {
        Self::Command {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        }
    }
}

/// A handle to run Mercurial commands with.
#[derive(Debug)]
pub struct Hg {
    repo_path: PathBuf,
}

impl Hg {
    /// Create an object to run commands on the passed repository.
    pub fn new<P: Into<PathBuf>>(repo_path: P) -> Self {
        Self {
            repo_path: repo_path.into(),
        }
    }

    async fn run_command<I, S>(&self, args: I) -> Result<String>
    where
        I: IntoIterator<Item = S> + Send,
        S: AsRef<OsStr>,
    {
        let output = Command::new("hg")
            .arg("-R")
            .arg(&self.repo_path)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await?;

        if output.status.success() {
            Ok(String::from_utf8(output.stdout)?)
        } else {
            Err(Error::command_error(&output))
        }
    }

    /// Pull changesets from the remote server without modifying the working directory.
    ///
    /// # Errors
    /// Returns an error if Mercurial fails to pull new revisions.
    pub async fn pull(&self) -> Result<()> {
        self.run_command(vec!["pull"]).await?;
        Ok(())
    }

    /// Get a list of revisions from the repository, optionally matching some revspec.
    ///
    /// # Errors
    /// Returns an error if Mercurial fails to list revisions, or if the data
    /// from Mercurial cannot be parsed.
    pub async fn log(&self, revspec: Option<&str>) -> Result<Vec<Revision>> {
        let mut args = vec!["log", "--template", "json"];
        if let Some(rev) = &revspec {
            args.push("--rev");
            args.push(rev)
        }
        let output = self.run_command(args).await?;

        serde_json::from_str(&output).map_err(Error::RevisionParse)
    }

    /// Prune a revision from the repository, marking it as obsolete. Optionally
    /// mark another revision as having succeeded it.
    ///
    /// # Errors
    /// Returns an error if Mercurial fails to prune the revision.
    pub async fn prune(&self, rev: &str, successor: Option<&str>) -> Result<()> {
        let mut args = vec!["prune", "--ref", rev];
        if let Some(succ) = successor {
            args.push("--succ");
            args.push(succ);
        }
        self.run_command(args).await?;
        Ok(())
    }
}

/// A revision in a Mercurial repository.
#[derive(Debug, Deserialize)]
pub struct Revision {
    /// The entire body of the revision comment
    #[serde(rename = "desc")]
    pub description: String,

    /// The global identifying hash of the revision.
    #[serde(rename = "node")]
    pub hash: String,
}

impl Revision {
    /// Extract the subject of the revision from the description, defined as the first line of the description.
    #[must_use]
    pub fn subject(&self) -> Option<&str> {
        let mut parts = self.description.split('\n');
        parts.next()
    }

    /// Get the bug listed in the revision subject, if any.
    #[must_use]
    pub fn bug(&self) -> Option<Bug> {
        let mut words = self.subject()?.split_whitespace();
        let first = words.next()?;
        if first.to_lowercase() == "bug" {
            words.next().map(|second| Bug::new(second.to_string()))
        } else {
            None
        }
    }
}
