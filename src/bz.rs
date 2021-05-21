//! An abstraction to interact with the Bugzilla API.

use anyhow::Context;
use serde::Deserialize;
use std::collections::HashMap;
use thiserror::Error;

const BZ_API: &str = "https://bugzilla.mozilla.org/rest";

/// A problem that prevents usage of the Bugzilla API.
#[derive(Error, Debug)]
pub enum Error {
    /// The API did not return the expected information
    #[error("The API did not return the expected information")]
    ApiContract,

    /// Could not complete API request
    #[error("Could not complete API request")]
    Http(#[from] reqwest::Error),

    /// Errors from `anyhow` are passed through transparently.
    #[error(transparent)]
    Anyhow(#[from] anyhow::Error),
}

type Result<T> = std::result::Result<T, Error>;

/// A Bugzilla bug.
#[derive(Debug)]
pub struct Bug {
    /// The ID of the bug.
    pub id: String,
}

impl Bug {
    /// Create a new bug.
    #[must_use]
    pub const fn new(id: String) -> Self {
        Self { id }
    }

    /// Bind an HTTP client to this bug so that more information can be pulled from the API.
    #[must_use]
    #[allow(clippy::missing_const_for_fn)] // Since this drops `self`, it in fact cannot be a `const fn`.
    pub fn with_api(self, client: &reqwest::Client) -> ApiBug {
        ApiBug::new(client, self.id)
    }
}

/// More detailed information about a bug pulled from the API.
#[allow(missing_copy_implementations)]
#[derive(Debug, Deserialize)]
pub struct BugDetail {
    /// The status of the bug, such as RESOLVED, or NEW.
    pub status: BugStatus,
}

/// The status of a bug, such as RESOLVED, or NEW.
#[derive(Copy, Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "UPPERCASE")]
pub enum BugStatus {
    /// This bug has recently been added to the list of bugs.
    New,

    /// A resolution has been performed, and it is awaiting verification.
    Resolved,

    /// The resolution of the bug has been verified.
    Verified,
}

/// A comment posted on a bug.
#[derive(Debug, Deserialize)]
pub struct Comment {
    /// The global ID of the comment.
    pub id: u32,
    /// The unformatted text of the comment.
    pub raw_text: String,
}

/// A Bugzilla bug that has been associated with an HTTP client for further API queries.
#[derive(Debug)]
pub struct ApiBug<'a> {
    /// The ID of the bug.
    pub id: String,

    client: &'a reqwest::Client,
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
struct BugComments {
    comments: Vec<Comment>,
}

impl<'a> ApiBug<'a> {
    const fn new(client: &'a reqwest::Client, id: String) -> Self {
        Self { id, client }
    }

    /// Fetch the details of this bug from the API.
    ///
    /// # Errors
    /// Returns an error if the API request fails or cannot be parsed.
    pub async fn details(&self) -> Result<BugDetail> {
        let url = format!("{}/bug/{}", BZ_API, self.id);
        let res = self.client.get(url).send().await?;
        let mut data: ApiListResponse<BugDetail> = res
            .json()
            .await
            .context(format!("Failed to fetch details for bug {}", self.id))?;
        Ok(data
            .bugs
            .pop()
            .ok_or(Error::ApiContract)
            .context("No bugs in response")?)
    }

    /// Fetch all comments on this bug from the API.
    ///
    /// # Errors
    /// Returns an error if the API request fails or cannot be parsed.
    pub async fn comments(&self) -> Result<Vec<Comment>> {
        let url = format!("{}/bug/{}/comment", BZ_API, self.id);
        let res = self.client.get(url).send().await?;
        let mut data: ApiMapResponse<BugComments> = res
            .json()
            .await
            .context(format!("Failed to fetch comments for bug {}", self.id))?;
        Ok(data
            .bugs
            .remove(&self.id)
            .ok_or(Error::ApiContract)
            .context("API fault: requested bug not in response")?
            .comments)
    }
}
