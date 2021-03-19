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
use bz::ApiBug;
use clap::Clap;
use futures::stream::{self, StreamExt, TryStreamExt};
use hg::Revision;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU32, Ordering},
};

#[derive(Clap)]
struct Opts {
    #[clap(short, long, default_value = ".")]
    path: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    let opts = Opts::parse();

    let hg = &Hg::new(&opts.path);

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

    // Prepare an HTTP client to attach to bugs
    let client = reqwest::Client::new();
    // Set up a counter for how many prunable revisions are found
    let num_prunable = AtomicU32::new(0);

    // For every revision, look for a bug number in the revision and then scan
    // that bug for any comments that indicate the draft has merged.
    //
    // The intent is that once a revision that appears prunable is found, the
    // user will be prompted immediately. At the same time, the search will
    // continue. The time the user spends considering the choice will be used to
    // continue searching for more prunable revisions.
    stream::iter(revs)
        // Add an API to every bug
        .filter_map(|rev: Revision| async { rev.bug().map(|bug| Ok((rev, bug.with_api(&client)))) })
        // Remove bugs thats aren't resolved or verified
        .try_filter_map(|(rev, bug)| async {
            let details = bug.details().await?;
            let v: Result<_, anyhow::Error> =
                if details.status == BugStatus::Resolved || details.status == BugStatus::Verified {
                    Ok(Some((rev, bug)))
                } else {
                    Ok(None)
                };
            v
        })
        // Find bugs that mention a merge to mozilla-central, starting with the oldest
        .try_filter_map(|(rev, bug): (Revision, ApiBug)| async move {
            let mut comments = bug.comments().await?;
            comments.reverse();
            for comment in comments {
                if comment
                    .raw_text
                    .starts_with("https://hg.mozilla.org/mozilla-central/rev/")
                {
                    let hash = comment.raw_text.split('/').last().unwrap();
                    if hash.chars().all(|c| c.is_ascii_hexdigit()) {
                        return Ok(Some((rev, hash.to_string())));
                    }
                }
            }
            Ok(None)
        })
        // For each prunable revision, prompt the user if it should be pruned.
        .try_filter_map(|(revision, successor)| async {
            num_prunable.fetch_add(1, Ordering::SeqCst);

            let stdin = io::stdin();
            let mut stdout = io::stdout();
            let mut buffer = String::new();

            print!(
                "{} {}\n  prune to {}? ",
                &revision.hash[..12],
                revision.subject().unwrap_or("<no description>"),
                successor
            );
            loop {
                print!("[Yn] > ");
                stdout.flush().await?;
                stdin.read_line(&mut buffer).await?;
                match buffer.trim().to_lowercase().as_str() {
                    "y" | "" => {
                        return Ok(Some((revision.hash, successor)));
                    }
                    "n" => return Ok(None),
                    _ => (),
                }
            }
        })
        // And finally prune the revisions
        .try_for_each(|(hash, successor)| async move {
            hg.prune(&hash, Some(&successor)).await?;
            Ok(())
        })
        .await?;

    if num_prunable.into_inner() == 0 {
        println!("No prunable revisions found");
    }

    Ok(())
}
