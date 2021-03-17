use crate::bz::Bug;
use async_std::{
    io,
    path::PathBuf,
    process::{Command, Output, Stdio},
};
use serde::Deserialize;
use std::ffi::OsStr;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum HgError {
    #[error("Could not run command")]
    IoError(#[from] io::Error),

    #[error("Mercurial error {stdout}{stderr}")]
    CommandError { stdout: String, stderr: String },

    #[error("Output was not valid UTF-8")]
    Utf8Error(#[from] std::string::FromUtf8Error),

    #[error("Mercurial output could not be parsed")]
    RevisionParseError(#[from] serde_json::error::Error),
}

type Result<T> = std::result::Result<T, HgError>;

impl HgError {
    fn command_error(output: Output) -> Self {
        Self::CommandError {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        }
    }
}

pub struct Hg {
    repo_path: PathBuf,
}

impl Hg {
    pub fn new<P: Into<PathBuf>>(repo_path: P) -> Self {
        Self {
            repo_path: repo_path.into(),
        }
    }

    async fn run_command<I, S>(&self, args: I) -> Result<String>
    where
        I: IntoIterator<Item = S>,
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

        if !output.status.success() {
            Err(HgError::command_error(output))
        } else {
            Ok(String::from_utf8(output.stdout)?)
        }
    }

    pub async fn pull(&self) -> Result<()> {
        self.run_command(vec!["pull"]).await?;
        Ok(())
    }

    pub async fn log(&self, rev: Option<&str>) -> Result<Vec<Revision>> {
        let mut args = vec!["log", "--template", "json"];
        if let Some(rev) = &rev {
            args.push("--rev");
            args.push(rev)
        }
        let output = self.run_command(args).await?;

        serde_json::from_str(&output).map_err(|err| HgError::RevisionParseError(err))
    }

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

#[derive(Debug, Deserialize)]
pub struct Revision {
    pub desc: String,

    #[serde(rename = "node")]
    pub hash: String,
}

impl Revision {
    pub fn header(&self) -> Option<&str> {
        let mut parts = self.desc.split("\n");
        parts.next()
    }

    pub fn bug(&self) -> Option<Bug> {
        let mut words = self.header()?.split_whitespace();
        let first = words.next()?;
        if first.to_lowercase() == "bug" {
            words.next().map(|second| Bug::new(second.to_string()))
        } else {
            None
        }
    }
}
