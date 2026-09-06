//! Gitea REST API client (CC-03, CC-32, CC-34, CC-41, CC-42, CC-44).
//!
//! Mutations in joinedcontext commit to Git merge requests server-side. Commits are attributed
//! to the signed-in human author, while the service token only authenticates the HTTP call.

use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::error::ApiError;

/// Gitea repository client.
#[derive(Clone)]
pub struct GiteaClient {
    pub base: Url,
    pub owner: String,
    pub repo: String,
    token: String,
    pub http: reqwest::Client,
}

impl std::fmt::Debug for GiteaClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GiteaClient")
            .field("base", &self.base.as_str())
            .field("owner", &self.owner)
            .field("repo", &self.repo)
            .field("token", &"[redacted]")
            .finish()
    }
}

/// Errors returned by the Gitea client.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GitError {
    #[error("git config error: {0}")]
    Config(String),
    #[error("git transport error: {0}")]
    Transport(String),
    #[error("git resource not found")]
    NotFound,
    #[error("git conflict: {0}")]
    Conflict(String),
    #[error("git api error {status}: {message}")]
    Api { status: u16, message: String },
}

impl From<GitError> for ApiError {
    fn from(err: GitError) -> Self {
        match err {
            GitError::NotFound => {
                ApiError::NotFound("resource not found in git repository".to_string())
            }
            GitError::Conflict(msg) => ApiError::Conflict(msg),
            other => {
                tracing::error!(error = %other, "git error");
                ApiError::Internal("git operation failed".to_string())
            }
        }
    }
}

/// Human author attributed on commits (CC-44).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Author<'a> {
    pub name: &'a str,
    pub email: &'a str,
}

/// File content read from repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoFile {
    pub sha: String,
    pub content: String,
}

/// File write specification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileWrite<'a> {
    pub path: &'a str,
    pub branch: &'a str,
    pub message: &'a str,
    pub content: &'a str,
    pub sha: Option<&'a str>,
    pub author: Author<'a>,
}

/// File deletion specification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDelete<'a> {
    pub path: &'a str,
    pub branch: &'a str,
    pub message: &'a str,
    pub sha: &'a str,
    pub author: Author<'a>,
}

/// Pull request representation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequest {
    pub number: u64,
    pub url: String,
    pub state: String,
    pub title: String,
    pub body: String,
    pub head_branch: String,
    pub base_branch: String,
    pub created_at: String,
    pub author_name: String,
    pub author_email: Option<String>,
    pub mergeable: Option<bool>,
    pub merged: bool,
}

/// Pull request review action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReviewEvent {
    #[serde(rename = "APPROVED")]
    Approve,
    #[serde(rename = "REQUEST_CHANGES")]
    RequestChanges,
    #[serde(rename = "COMMENT")]
    Comment,
}

/// Pull request merge style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MergeStyle {
    Merge,
    Squash,
    Rebase,
}

// Request and response DTOs for Gitea REST API
#[derive(Deserialize)]
struct RepoDetailsResponse {
    default_branch: String,
}

#[derive(Deserialize)]
struct BranchDetailsResponse {
    commit: CommitIdDto,
}

#[derive(Deserialize)]
struct CommitIdDto {
    id: String,
}

#[derive(Serialize)]
struct CreateBranchPayload<'a> {
    new_branch_name: &'a str,
    old_branch_name: &'a str,
}

#[derive(Deserialize)]
struct ContentsDto {
    sha: String,
    #[serde(default)]
    content: String,
}

#[derive(Serialize)]
struct PutFilePayload<'a> {
    content: String,
    message: &'a str,
    branch: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    sha: Option<&'a str>,
    author: Author<'a>,
    committer: Author<'a>,
}

#[derive(Serialize)]
struct DeleteFilePayload<'a> {
    message: &'a str,
    branch: &'a str,
    sha: &'a str,
    author: Author<'a>,
    committer: Author<'a>,
}

#[derive(Deserialize)]
struct FileCommitResponse {
    #[serde(default)]
    commit: Option<FileCommitShaDto>,
    #[serde(default)]
    sha: Option<String>,
}

#[derive(Deserialize)]
struct FileCommitShaDto {
    sha: String,
}

#[derive(Serialize)]
struct CreatePullRequestPayload<'a> {
    head: &'a str,
    base: &'a str,
    title: &'a str,
    body: &'a str,
}

#[derive(Deserialize)]
struct GiteaPullResponse {
    number: u64,
    html_url: String,
    state: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    head: Option<BranchRefDto>,
    #[serde(default)]
    base: Option<BranchRefDto>,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    user: Option<GiteaUserDto>,
    #[serde(default)]
    mergeable: Option<bool>,
    #[serde(default)]
    merged: bool,
}

#[derive(Deserialize)]
struct BranchRefDto {
    #[serde(rename = "ref", default)]
    git_ref: String,
}

#[derive(Deserialize)]
struct GiteaUserDto {
    #[serde(default)]
    login: Option<String>,
    #[serde(default)]
    full_name: Option<String>,
    #[serde(default)]
    email: Option<String>,
}

impl From<GiteaPullResponse> for PullRequest {
    fn from(raw: GiteaPullResponse) -> Self {
        let author_name = raw
            .user
            .as_ref()
            .and_then(|u| {
                u.full_name
                    .as_deref()
                    .filter(|s| !s.trim().is_empty())
                    .or(u.login.as_deref())
            })
            .unwrap_or_default()
            .to_string();
        let author_email = raw.user.and_then(|u| u.email);
        let head_branch = raw.head.map(|h| h.git_ref).unwrap_or_default();
        let base_branch = raw.base.map(|b| b.git_ref).unwrap_or_default();

        PullRequest {
            number: raw.number,
            url: raw.html_url,
            state: raw.state,
            title: raw.title,
            body: raw.body.unwrap_or_default(),
            head_branch,
            base_branch,
            created_at: raw.created_at.unwrap_or_default(),
            author_name,
            author_email,
            mergeable: raw.mergeable,
            merged: raw.merged,
        }
    }
}

#[derive(Serialize)]
struct ReviewPayload<'a> {
    event: ReviewEvent,
    body: &'a str,
}

#[derive(Serialize)]
struct MergePayload<'a> {
    #[serde(rename = "Do")]
    do_field: MergeStyle,
    merge_message_field: &'a str,
}

#[derive(Deserialize)]
struct GitTreeResponse {
    #[serde(default)]
    tree: Vec<GitTreeEntryDto>,
    #[serde(default)]
    truncated: bool,
}

#[derive(Deserialize)]
struct GitTreeEntryDto {
    path: String,
    #[serde(rename = "type")]
    entry_type: String,
}

impl GiteaClient {
    pub fn new(
        base: Url,
        owner: impl Into<String>,
        repo: impl Into<String>,
        token: impl Into<String>,
    ) -> Result<Self, GitError> {
        let http = reqwest::ClientBuilder::new()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| GitError::Transport(e.to_string()))?;

        Ok(Self {
            base,
            owner: owner.into(),
            repo: repo.into(),
            token: token.into(),
            http,
        })
    }

    /// Reads Gitea configuration from environment variables.
    /// Fail-closed: returns `Ok(None)` if all 4 are absent, or an error if partially set.
    pub fn from_env(lookup: impl Fn(&str) -> Option<String>) -> Result<Option<Self>, GitError> {
        match (
            lookup("JC_GITEA_URL"),
            lookup("JC_GITEA_OWNER"),
            lookup("JC_GITEA_REPO"),
            lookup("JC_GITEA_TOKEN"),
        ) {
            (None, None, None, None) => Ok(None),
            (Some(url), Some(owner), Some(repo), Some(token)) => {
                let base = Url::parse(&url)
                    .map_err(|e| GitError::Config(format!("invalid JC_GITEA_URL: {e}")))?;
                Self::new(base, owner, repo, token).map(Some)
            }
            _ => Err(GitError::Config(
                "JC_GITEA_URL, JC_GITEA_OWNER, JC_GITEA_REPO and JC_GITEA_TOKEN must be set together"
                    .to_string(),
            )),
        }
    }

    /// Browser URL of one file at a git ref, the page a "Source" link opens (never the API URL).
    pub fn browse_url(&self, path: &str, git_ref: &str) -> String {
        format!(
            "{}/{}/{}/src/branch/{}/{}",
            self.base.as_str().trim_end_matches('/'),
            self.owner,
            self.repo,
            git_ref,
            path.trim_start_matches('/'),
        )
    }

    fn repo_url(&self, path: &str) -> Result<Url, GitError> {
        let clean_base = self.base.as_str().trim_end_matches('/');
        let path = path.trim_start_matches('/');
        let full = if path.is_empty() {
            format!("{clean_base}/api/v1/repos/{}/{}", self.owner, self.repo)
        } else {
            format!(
                "{clean_base}/api/v1/repos/{}/{}/{path}",
                self.owner, self.repo
            )
        };
        Url::parse(&full).map_err(|e| GitError::Config(format!("invalid url '{full}': {e}")))
    }

    async fn send(&self, builder: reqwest::RequestBuilder) -> Result<reqwest::Response, GitError> {
        builder
            .header(
                reqwest::header::AUTHORIZATION,
                format!("token {}", self.token),
            )
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|e| GitError::Transport(e.to_string()))
    }

    async fn check_status(res: reqwest::Response) -> Result<reqwest::Response, GitError> {
        let status = res.status();
        if status.is_success() {
            Ok(res)
        } else if status == reqwest::StatusCode::NOT_FOUND {
            Err(GitError::NotFound)
        } else if status == reqwest::StatusCode::CONFLICT {
            let message = res.text().await.unwrap_or_default();
            Err(GitError::Conflict(message))
        } else {
            let status_code = status.as_u16();
            let message = res.text().await.unwrap_or_default();
            Err(GitError::Api {
                status: status_code,
                message,
            })
        }
    }

    /// `GET ""` — returns the repository's default branch.
    pub async fn default_branch(&self) -> Result<String, GitError> {
        let url = self.repo_url("")?;
        let res = self.send(self.http.get(url)).await?;
        let res = Self::check_status(res).await?;
        let repo: RepoDetailsResponse = res.json().await.map_err(|e| {
            GitError::Transport(format!("failed to parse repository response: {e}"))
        })?;
        Ok(repo.default_branch)
    }

    /// `GET /git/trees/{git_ref}?recursive=true&per_page=1000` — retrieves the Git tree.
    pub async fn list_tree(&self, git_ref: &str) -> Result<Vec<String>, GitError> {
        let mut url = self.repo_url(&format!("git/trees/{git_ref}"))?;
        url.query_pairs_mut()
            .append_pair("recursive", "true")
            .append_pair("per_page", "1000");

        let res = self.send(self.http.get(url)).await?;
        let res = Self::check_status(res).await?;
        let status_code = res.status().as_u16();
        let raw: GitTreeResponse = res
            .json()
            .await
            .map_err(|e| GitError::Transport(format!("failed to parse tree response: {e}")))?;

        if raw.truncated {
            return Err(GitError::Api {
                status: status_code,
                message: "git tree was truncated".to_string(),
            });
        }

        let paths = raw
            .tree
            .into_iter()
            .filter(|entry| entry.entry_type == "blob")
            .map(|entry| entry.path)
            .collect();

        Ok(paths)
    }

    /// `GET /branches/{branch}` — returns the head commit id for the given branch.
    pub async fn branch_head(&self, branch: &str) -> Result<String, GitError> {
        let url = self.repo_url(&format!("branches/{branch}"))?;
        let res = self.send(self.http.get(url)).await?;
        let res = Self::check_status(res).await?;
        let branch_dto: BranchDetailsResponse = res
            .json()
            .await
            .map_err(|e| GitError::Transport(format!("failed to parse branch response: {e}")))?;
        Ok(branch_dto.commit.id)
    }

    /// `POST /branches` — creates a new branch from an existing one.
    pub async fn create_branch(&self, new_branch: &str, from_branch: &str) -> Result<(), GitError> {
        let url = self.repo_url("branches")?;
        let payload = CreateBranchPayload {
            new_branch_name: new_branch,
            old_branch_name: from_branch,
        };
        let res = self.send(self.http.post(url).json(&payload)).await?;
        Self::check_status(res).await?;
        Ok(())
    }

    /// `GET /contents/{path}?ref={git_ref}` — reads a file and decodes its base64 content.
    pub async fn get_file(&self, path: &str, git_ref: &str) -> Result<Option<RepoFile>, GitError> {
        let mut url = self.repo_url(&format!("contents/{}", path.trim_start_matches('/')))?;
        url.query_pairs_mut().append_pair("ref", git_ref);

        let res = self.send(self.http.get(url)).await?;
        if res.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let res = Self::check_status(res).await?;
        let raw: ContentsDto = res
            .json()
            .await
            .map_err(|e| GitError::Transport(format!("failed to parse contents response: {e}")))?;

        let clean_base64: String = raw.content.chars().filter(|c| !c.is_whitespace()).collect();
        let decoded_bytes = STANDARD
            .decode(clean_base64.as_bytes())
            .map_err(|e| GitError::Transport(format!("invalid base64 content: {e}")))?;
        let content = String::from_utf8(decoded_bytes)
            .map_err(|e| GitError::Transport(format!("content is not valid utf-8: {e}")))?;

        Ok(Some(RepoFile {
            sha: raw.sha,
            content,
        }))
    }

    /// `PUT /contents/{path}` — creates or replaces a file with human commit attribution.
    pub async fn put_file(&self, req: &FileWrite<'_>) -> Result<String, GitError> {
        let url = self.repo_url(&format!("contents/{}", req.path.trim_start_matches('/')))?;
        let encoded = STANDARD.encode(req.content.as_bytes());
        let payload = PutFilePayload {
            content: encoded,
            message: req.message,
            branch: req.branch,
            sha: req.sha,
            author: req.author,
            committer: req.author,
        };
        let res = self.send(self.http.put(url).json(&payload)).await?;
        let res = Self::check_status(res).await?;
        let body: FileCommitResponse = res
            .json()
            .await
            .map_err(|e| GitError::Transport(format!("failed to parse commit response: {e}")))?;
        let commit_sha = body
            .commit
            .map(|c| c.sha)
            .or(body.sha)
            .ok_or_else(|| GitError::Transport("missing commit sha in response".to_string()))?;
        Ok(commit_sha)
    }

    /// `DELETE /contents/{path}` — deletes a file with human commit attribution.
    pub async fn delete_file(&self, req: &FileDelete<'_>) -> Result<String, GitError> {
        let url = self.repo_url(&format!("contents/{}", req.path.trim_start_matches('/')))?;
        let payload = DeleteFilePayload {
            message: req.message,
            branch: req.branch,
            sha: req.sha,
            author: req.author,
            committer: req.author,
        };
        let res = self.send(self.http.delete(url).json(&payload)).await?;
        let res = Self::check_status(res).await?;
        let body: Result<FileCommitResponse, _> = res.json().await;
        let commit_sha = match body {
            Ok(b) => b.commit.map(|c| c.sha).or(b.sha).unwrap_or_default(),
            Err(_) => String::new(),
        };
        Ok(commit_sha)
    }

    /// `GET /pulls?state={state}&sort=recentupdate&limit=50` — retrieves pull requests.
    pub async fn list_pull_requests(&self, state: &str) -> Result<Vec<PullRequest>, GitError> {
        let mut url = self.repo_url("pulls")?;
        url.query_pairs_mut()
            .append_pair("state", state)
            .append_pair("sort", "recentupdate")
            .append_pair("limit", "50");

        let res = self.send(self.http.get(url)).await?;
        let res = Self::check_status(res).await?;
        let raw_list: Vec<GiteaPullResponse> = res
            .json()
            .await
            .map_err(|e| GitError::Transport(format!("failed to parse pull requests list: {e}")))?;
        Ok(raw_list.into_iter().map(Into::into).collect())
    }

    /// `POST /pulls` — creates a new pull request.
    pub async fn create_pull_request(
        &self,
        head: &str,
        base: &str,
        title: &str,
        body: &str,
    ) -> Result<PullRequest, GitError> {
        let url = self.repo_url("pulls")?;
        let payload = CreatePullRequestPayload {
            head,
            base,
            title,
            body,
        };
        let res = self.send(self.http.post(url).json(&payload)).await?;
        let res = Self::check_status(res).await?;
        let raw: GiteaPullResponse = res
            .json()
            .await
            .map_err(|e| GitError::Transport(format!("failed to parse pull request: {e}")))?;
        Ok(raw.into())
    }

    /// `GET /pulls/{number}` — retrieves an existing pull request.
    pub async fn pull_request(&self, number: u64) -> Result<PullRequest, GitError> {
        let url = self.repo_url(&format!("pulls/{number}"))?;
        let res = self.send(self.http.get(url)).await?;
        let res = Self::check_status(res).await?;
        let raw: GiteaPullResponse = res
            .json()
            .await
            .map_err(|e| GitError::Transport(format!("failed to parse pull request: {e}")))?;
        Ok(raw.into())
    }

    /// `POST /pulls/{number}/reviews` — submits a review on the pull request.
    pub async fn review(
        &self,
        number: u64,
        event: ReviewEvent,
        body: &str,
    ) -> Result<(), GitError> {
        let url = self.repo_url(&format!("pulls/{number}/reviews"))?;
        let payload = ReviewPayload { event, body };
        let res = self.send(self.http.post(url).json(&payload)).await?;
        Self::check_status(res).await?;
        Ok(())
    }

    /// `POST /pulls/{number}/merge` — merges the pull request.
    pub async fn merge(
        &self,
        number: u64,
        style: MergeStyle,
        message: &str,
    ) -> Result<(), GitError> {
        let url = self.repo_url(&format!("pulls/{number}/merge"))?;
        let payload = MergePayload {
            do_field: style,
            merge_message_field: message,
        };
        let res = self.send(self.http.post(url).json(&payload)).await?;
        Self::check_status(res).await?;
        Ok(())
    }
}
