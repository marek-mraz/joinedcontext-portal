//! One reviewable change, written onto a branch and opened as a merge request.
//!
//! Both schedules the reconciler drives end here: the foreign-model mirror (DM-49) and the
//! `SyncSource` loop (MF-28, MF-29). Neither of them writes to the branch the platform
//! applies. `jcctl` decides *what* the change is and this module is the only place that
//! commits it, so there is one answer to "how does the reconciler propose something" rather
//! than one per schedule.
//!
//! A branch is reused rather than recreated: the names both callers pass are deterministic for
//! the revision they carry, so a run that failed halfway through the forge calls continues on
//! the branch it started instead of leaving one behind per attempt (CC-18).

use std::collections::BTreeMap;

use crate::git::{Author, FileDelete, FileWrite, GitError, GiteaClient, MergeStyle, PullRequest};

/// Who the forge records as the author of an automatic proposal.
///
/// Not a person and never a person's token: a reviewer looking at the history has to be able
/// to tell a commit somebody made from a commit a schedule made (MF-30).
const AUTHOR_NAME: &str = "joinedcontext reconciler";
const AUTHOR_EMAIL: &str = "reconciler@joinedcontext.local";

/// What to put on a branch, and what to say about it.
pub struct Proposal<'a> {
    /// Branch name, deterministic for the change it carries.
    pub branch: &'a str,
    /// Merge request title.
    pub title: &'a str,
    /// Merge request body: the plan a reviewer reads.
    pub body: &'a str,
    /// Files to write, by repository-relative path.
    pub files: BTreeMap<String, String>,
    /// Repository paths to remove, when the source no longer carries them (CC-19).
    pub removed: Vec<String>,
    /// Whether the proposal may merge itself once it is open (MF-29, CC-70).
    pub auto_merge: bool,
}

/// Writes the proposal onto its branch and opens the merge request (MF-29, DM-49).
///
/// The merge request is opened even when `auto_merge` is set: the merge is a second step on a
/// request that exists, so what merged is in the forge's history either way.
pub async fn open(gitea: &GiteaClient, proposal: &Proposal<'_>) -> Result<PullRequest, GitError> {
    let default_branch = gitea.default_branch().await?;
    create_or_reuse(gitea, proposal.branch, &default_branch).await?;

    let author = Author {
        name: AUTHOR_NAME,
        email: AUTHOR_EMAIL,
    };
    for (path, content) in &proposal.files {
        // The sha of what the branch already holds, so a retry updates the file instead of
        // being refused for creating one that exists.
        let existing = gitea
            .get_file(path, proposal.branch)
            .await
            .ok()
            .flatten()
            .map(|file| file.sha);
        gitea
            .put_file(&FileWrite {
                path,
                branch: proposal.branch,
                message: &format!("{}: {path}", proposal.title),
                content,
                sha: existing.as_deref(),
                author,
            })
            .await?;
    }

    for path in &proposal.removed {
        // A path the source dropped and the repository never had is not an error: the run
        // that dropped it may have got this far before and failed on a later file.
        let Some(existing) = gitea.get_file(path, proposal.branch).await.ok().flatten() else {
            continue;
        };
        gitea
            .delete_file(&FileDelete {
                path,
                branch: proposal.branch,
                message: &format!("{}: remove {path}", proposal.title),
                sha: &existing.sha,
                author,
            })
            .await?;
    }

    let pull = gitea
        .create_pull_request(
            proposal.branch,
            &default_branch,
            proposal.title,
            proposal.body,
        )
        .await?;

    if proposal.auto_merge {
        gitea
            .merge(pull.number, MergeStyle::Merge, proposal.title)
            .await?;
    }
    Ok(pull)
}

/// The branch, whether or not a previous attempt already created it.
async fn create_or_reuse(
    gitea: &GiteaClient,
    branch: &str,
    default_branch: &str,
) -> Result<(), GitError> {
    match gitea.create_branch(branch, default_branch).await {
        Ok(()) => Ok(()),
        Err(GitError::Conflict(_)) => {
            tracing::info!(branch = %branch, "continuing on the branch an earlier run started");
            Ok(())
        }
        Err(other) => Err(other),
    }
}
