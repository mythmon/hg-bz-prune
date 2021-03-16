use std::ffi::OsStr;

use async_std::{
    io,
    path::PathBuf,
    process::{Command, Output, Stdio},
};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum HgError {
    #[error("Could not run command")]
    IoError(#[from] io::Error),

    #[error("Mercurial error {stdout}{stderr}")]
    CommandError { stdout: String, stderr: String },

    #[error("Output was not valid UTF-8")]
    Utf8Error(#[from] std::string::FromUtf8Error),
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

    pub async fn run<I, S>(&self, args: I) -> Result<String>
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
}
