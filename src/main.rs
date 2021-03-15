use anyhow::{anyhow, bail, Context, Result};
use async_std::{
    io::{self, prelude::WriteExt},
    process::{Command, Stdio},
};
use clap::Clap;
use serde::Deserialize;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

const BZ_API: &'static str = "https://bugzilla.mozilla.org/rest";

#[derive(Clap)]
struct Opts {
    #[clap(
        short,
        long,
        default_value = "/home/mythmon/src/mozilla-unified-artifact"
    )]
    path: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    let opt = Opts::parse();

    // Start a background pull
    let pull_future = Command::new("hg")
        .arg("-R")
        .arg(&opt.path)
        .arg("pull")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();

    // Get draft revisions
    let output = Command::new("hg")
        .arg("-R")
        .arg(&opt.path)
        .arg("log")
        .args(vec!["--rev", "draft() and not(obsolete())"])
        .args(vec!["--template", "json"])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .output()
        .await
        .context("Failed to get list of draft revisions")?;

    if !output.status.success() {
        bail!("Mercurial error {}", String::from_utf8(output.stdout)?);
    }

    let revs: Vec<Revision> = serde_json::from_str(
        &String::from_utf8(output.stdout).context("Mercurial output was not utf8")?,
    )
    .context("Could not parse revision information")?;

    if revs.is_empty() {
        println!("No draft revisions found");
        return Ok(());
    }

    // Find any comments in the bug that indicate the draft has merged
    let client = reqwest::Client::new();
    let mut prunable = vec![];
    for rev in &revs {
        if let Some(bug) = rev.bug() {
            let bug = bug.with_api(&client);
            let details = bug.details().await?;
            if details.status == BugStatus::Resolved || details.status == BugStatus::Verified {
                let comments = bug.comments().await?;
                for comment in comments {
                    if comment
                        .raw_text
                        .starts_with("https://hg.mozilla.org/mozilla-central/rev/")
                    {
                        let hash = comment.raw_text.split('/').last().unwrap();
                        if hash.chars().all(|c| c.is_ascii_hexdigit()) {
                            prunable.push((rev, hash.to_string()));
                        }
                    }
                }
            }
        }
    }

    // Wait for the pull to finish
    let pull_status = pull_future.await;
    match pull_status {
        Err(err) => println!("Warning, pull failed: {}", err),
        Ok(output) if !output.status.success() => println!(
            "Warning, pull failed: {}{}",
            String::from_utf8(output.stdout)?,
            String::from_utf8(output.stderr)?
        ),
        Ok(_) => (),
    }

    // Ask to prune each revision
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut buffer = String::new();
    for (local, remote) in prunable {
        println!(
            "{} {}",
            &local.node[..12],
            local.header().unwrap_or("<no description>")
        );
        print!("  prune to {}? ", remote);
        loop {
            print!("[Yn] > ");
            stdout.flush().await?;
            stdin.read_line(&mut buffer).await?;
            match buffer.trim() {
                "y" | "Y" | "" => {
                    prune_revision(&opt.path, &local.node, &remote).await?;
                    break;
                }
                _ => (),
            }
        }
    }

    Ok(())
}

async fn prune_revision(path: &Path, rev: &str, succ: &str) -> Result<()> {
    let output = Command::new("hg")
        .arg("-R")
        .arg(path)
        .arg("prune")
        .args(vec!["--rev", &rev])
        .args(vec!["--succ", &succ])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .output()
        .await
        .context("Failed to get list of draft revisions")?;
    if !output.status.success() {
        bail!(
            "Prune failed: {}{}",
            String::from_utf8(output.stdout)?,
            String::from_utf8(output.stderr)?
        )
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct Revision {
    desc: String,
    node: String,
}

impl Revision {
    fn header(&self) -> Option<&str> {
        let mut parts = self.desc.split("\n");
        parts.next()
    }

    fn bug(&self) -> Option<Bug> {
        let mut words = self.header()?.split_whitespace();
        let first = words.next()?;
        if first.to_lowercase() == "bug" {
            words.next().map(|second| Bug::new(second.to_string()))
        } else {
            None
        }
    }
}

struct Bug {
    id: String,
}

impl Bug {
    fn new(id: String) -> Self {
        Self { id }
    }

    fn with_api<'a>(self, client: &'a reqwest::Client) -> ApiBug<'a> {
        ApiBug::new(client, self.id)
    }
}

struct ApiBug<'a> {
    client: &'a reqwest::Client,
    id: String,
}

impl<'a> ApiBug<'a> {
    fn new(client: &'a reqwest::Client, id: String) -> Self {
        Self { client, id }
    }

    async fn details(&self) -> Result<BugDetail> {
        let url = format!("{}/bug/{}", BZ_API, self.id);
        let res = self.client.get(url).send().await?;
        let mut data: ApiListResponse<BugDetail> = res
            .json()
            .await
            .context(format!("Failed to fetch details for bug {}", self.id))?;
        Ok(data
            .bugs
            .pop()
            .ok_or(anyhow!("API fault: no bugs in response"))?)
    }

    async fn comments(&self) -> Result<Vec<Comment>> {
        let url = format!("{}/bug/{}/comment", BZ_API, self.id);
        let res = self.client.get(url).send().await?;
        let mut data: ApiMapResponse<BugComments> = res
            .json()
            .await
            .context(format!("Failed to fetch comments for bug {}", self.id))?;
        Ok(data
            .bugs
            .remove(&self.id)
            .ok_or(anyhow!("API fault: requested bug not in response"))?
            .comments)
    }
}

#[derive(Debug, Deserialize)]
struct ApiMapResponse<T> {
    bugs: HashMap<String, T>,
}

#[derive(Debug, Deserialize)]
struct ApiListResponse<T> {
    bugs: Vec<T>,
}

#[derive(Debug, Deserialize)]
struct BugDetail {
    status: BugStatus,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "UPPERCASE")]
enum BugStatus {
    Resolved,
    Verified,
}

#[derive(Debug, Deserialize)]
struct BugComments {
    comments: Vec<Comment>,
}

#[derive(Debug, Deserialize)]
struct Comment {
    id: u32,
    raw_text: String,
}
