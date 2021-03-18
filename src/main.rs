//! A tool to automate pruning Mercurial revisions that have landed in mozilla-central.

#![deny(clippy::all, clippy::cargo, unsafe_code)]
#![warn(
    clippy::pedantic,
    clippy::nursery,
    missing_copy_implementations,
    missing_crate_level_docs,
    missing_debug_implementations,
    missing_docs,
    single_use_lifetimes,
    trivial_casts,
    trivial_numeric_casts,
    unreachable_pub,
    unsafe_code,
    unused_crate_dependencies,
    unused_import_braces,
    unused_qualifications,
    variant_size_differences
)]

pub mod bz;
pub mod hg;

use crate::{bz::BugStatus, hg::Hg};
use anyhow::{Context, Result};
use async_std::io::{self, prelude::WriteExt};
use clap::Clap;
use std::path::PathBuf;

#[derive(Clap)]
struct Opts {
    #[clap(short, long, default_value = ".")]
    path: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    let opts = Opts::parse();

    let hg = Hg::new(&opts.path);

    // Try to get up to date revisions, but don't fail if it doesn't work.
    if let Err(err) = hg.pull().await {
        println!("Warning, pull failed: {}", err);
    }

    // Get draft revisions
    let revs = hg
        .log(Some("draft() and not(obsolete())"))
        .await
        .context("Failed to get list of draft revisions")?;

    if revs.is_empty() {
        println!("No draft revisions found");
        return Ok(());
    }

    // For every revision, look for a bug number in the revision and then scan
    // that bug for any comments that indicate the draft has merged.
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

    if prunable.is_empty() {
        println!("No prunable revisions found");
        return Ok(());
    }

    // Ask to prune each revision
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut buffer = String::new();
    for (local, remote) in prunable {
        println!(
            "{} {}",
            &local.hash[..12],
            local.subject().unwrap_or("<no description>")
        );
        print!("  prune to {}? ", remote);
        loop {
            print!("[Yn] > ");
            stdout.flush().await?;
            stdin.read_line(&mut buffer).await?;
            match buffer.trim() {
                "y" | "Y" | "" => {
                    prune_revision(&hg, &local.hash, &remote).await?;
                    break;
                }
                _ => (),
            }
        }
    }

    Ok(())
}

async fn prune_revision(hg: &Hg, rev: &str, succ: &str) -> Result<()> {
    hg.prune(rev, Some(succ))
        .await
        .context("Failed to prune revision")?;
    Ok(())
}
