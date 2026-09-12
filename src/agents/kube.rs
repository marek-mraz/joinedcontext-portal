//! The one Job a builder run is, and the two objects that hold it in (AG-33…AG-35, AG-52).
//!
//! Three objects, all named `agent-run-{id}`: a ServiceAccount that mounts no token, a
//! NetworkPolicy that lets the workspace reach the credential proxy and DNS and nothing else,
//! and the Job itself. The workspace carries a run id and a ticket; every credential stays in
//! the proxy (ADR-N-020), so there is no Secret to mount and nothing in the pod spec that a
//! `kubectl get pod -o yaml` would have to be kept away from.

use crate::agents::profile::Profile;
use crate::agents::run::AgentRun;
use crate::apps::kube::KubeClient;

/// The label the credential proxy's pods carry. The proxy runs in the same namespace as the
/// workspaces it serves, which is what lets the egress rule be a pod selector rather than a
/// namespace the Portal would have to be told about.
const PROXY_POD_LABEL: &str = "jc-agent-proxy";
/// The label the Portal's own pods carry. The Portal drives the agent server over HTTP, so it
/// is the one peer allowed in.
const PORTAL_POD_LABEL: &str = "portal";
/// Where `openhands-agent-server` listens inside the workspace.
pub const AGENT_SERVER_PORT: u16 = 8000;
/// Where `jc-agent-proxy` listens.
pub const PROXY_PORT: u16 = 8080;

/// Schedules one run's workspace. `Ok(false)` means this Portal has no cluster to schedule in,
/// which is the development shape: the caller drives the agent server itself and is handed the
/// ticket in the answer.
#[allow(clippy::too_many_arguments)]
pub async fn schedule_workspace_job(
    kube: Option<&KubeClient>,
    namespace: &str,
    run: &AgentRun,
    ticket: &str,
    proxy_base: &str,
    profile: &Profile,
    ttl_secs: i64,
) -> Result<bool, String> {
    let Some(kube) = kube else {
        tracing::info!(
            run_id = %run.id,
            "no cluster: the workspace is not scheduled and the ticket goes to the caller"
        );
        return Ok(false);
    };

    let name = format!("agent-run-{}", run.id);
    let labels = serde_json::json!({
        "app.kubernetes.io/name": name,
        "app.kubernetes.io/part-of": "joinedcontext",
        "joinedcontext.com/run-id": run.id,
    });

    let service_account = serde_json::json!({
        "apiVersion": "v1",
        "kind": "ServiceAccount",
        "metadata": { "name": name, "namespace": namespace, "labels": labels },
        // AG-34: the workspace has no business talking to the Kubernetes API at all.
        "automountServiceAccountToken": false,
    });

    let network_policy = serde_json::json!({
        "apiVersion": "networking.k8s.io/v1",
        "kind": "NetworkPolicy",
        "metadata": { "name": name, "namespace": namespace, "labels": labels },
        "spec": {
            "podSelector": { "matchLabels": { "joinedcontext.com/run-id": run.id } },
            "policyTypes": ["Ingress", "Egress"],
            "ingress": [{
                // The Portal drives the agent server. Nothing else reaches the workspace, and
                // the workspace is not a Service, so nothing else can find it either.
                // ponytail: the pod label in any namespace, because a Portal outside the
                // agents namespace would otherwise have to tell the run its own namespace;
                // narrow it to a namespaceSelector when a second Portal shares the cluster.
                "from": [{
                    "namespaceSelector": {},
                    "podSelector": { "matchLabels": { "app.kubernetes.io/name": PORTAL_POD_LABEL } }
                }],
                "ports": [{ "protocol": "TCP", "port": AGENT_SERVER_PORT }]
            }],
            "egress": [
                {
                    "to": [{
                        "namespaceSelector": { "matchLabels": { "kubernetes.io/metadata.name": "kube-system" } },
                        "podSelector": { "matchLabels": { "k8s-app": "kube-dns" } }
                    }],
                    "ports": [
                        { "protocol": "UDP", "port": 53 },
                        { "protocol": "TCP", "port": 53 }
                    ]
                },
                {
                    // The credential proxy, by pod, in this namespace. A rule with no `to`
                    // would open the port on every address in the cluster, which is how a
                    // workspace would reach the broker or Keycloak directly (AG-40).
                    "to": [{
                        "podSelector": { "matchLabels": { "app.kubernetes.io/name": PROXY_POD_LABEL } }
                    }],
                    "ports": [{ "protocol": "TCP", "port": PROXY_PORT }]
                }
            ]
        }
    });

    let job = serde_json::json!({
        "apiVersion": "batch/v1",
        "kind": "Job",
        "metadata": { "name": name, "namespace": namespace, "labels": labels },
        "spec": {
            // One attempt: a builder run is a conversation, and replaying half of one against
            // a ticket that is already spent would only spend the budget twice (AG-41).
            "backoffLimit": 0,
            // The same number the run record expires on, so the pod and the record cannot
            // disagree about when the run is over (AG-43).
            "activeDeadlineSeconds": ttl_secs,
            "ttlSecondsAfterFinished": 600,
            "template": {
                "metadata": { "labels": labels },
                "spec": {
                    "restartPolicy": "Never",
                    "automountServiceAccountToken": false,
                    "serviceAccountName": name,
                    "enableServiceLinks": false,
                    "securityContext": {
                        "runAsNonRoot": true,
                        "runAsUser": 10001,
                        "fsGroup": 10001,
                        "seccompProfile": { "type": "RuntimeDefault" }
                    },
                    "containers": [{
                        "name": "agent-server",
                        "image": profile.image,
                        // The upstream image's own entrypoint, with the port and the session
                        // key this run alone knows. Nothing is built on top of it: a custom
                        // image would be a second thing to sign and pin (ADR-N-020).
                        "command": ["openhands-agent-server"],
                        "args": [
                            "--host", "0.0.0.0",
                            format!("--port={AGENT_SERVER_PORT}"),
                        ],
                        "securityContext": {
                            "readOnlyRootFilesystem": true,
                            "allowPrivilegeEscalation": false,
                            "capabilities": { "drop": ["ALL"] }
                        },
                        "resources": {
                            "requests": { "cpu": profile.cpu, "memory": profile.memory },
                            "limits": { "cpu": profile.cpu, "memory": profile.memory }
                        },
                        "ports": [{ "name": "agent", "containerPort": AGENT_SERVER_PORT }],
                        "env": [
                            // What the workspace may know: where the proxy is, which run it
                            // is, and the ticket that proves it. No model key, no forge
                            // token, no realm secret (AG-40).
                            { "name": "JC_PROXY_BASE", "value": proxy_base },
                            { "name": "JC_RUN_ID", "value": run.id },
                            { "name": "JC_RUN_TICKET", "value": ticket },
                            { "name": "JC_APP_NAME", "value": run.app_name },
                            { "name": "JC_APP_CLASS", "value": run.app_class },
                            { "name": "JC_BRANCH", "value": run.branch },
                            { "name": "JC_PATH_PREFIX", "value": run.path_prefix },
                            // The agent server's own API key, so only the Portal can drive it.
                            // It is the run ticket: one secret per run, minted once, and the
                            // proxy is what makes it worth nothing to anyone else.
                            { "name": "OH_SESSION_API_KEYS_0", "value": ticket },
                            // The model is reached through the proxy, which is the only place
                            // a provider key exists. The base URL is a workspace-visible
                            // address; the key is the ticket again.
                            { "name": "LLM_BASE_URL", "value": format!("{proxy_base}/v1/llm/v1") },
                            { "name": "LLM_API_KEY", "value": ticket },
                            { "name": "LLM_MODEL", "value": profile.model_name },
                            { "name": "HOME", "value": "/workspace" }
                        ],
                        "volumeMounts": [
                            { "name": "workspace", "mountPath": "/workspace" },
                            // `readOnlyRootFilesystem` leaves nowhere to write a temporary
                            // file, and every toolchain wants one.
                            { "name": "tmp", "mountPath": "/tmp" }
                        ]
                    }],
                    "volumes": [
                        { "name": "workspace", "emptyDir": { "sizeLimit": profile.ephemeral_storage } },
                        { "name": "tmp", "emptyDir": { "sizeLimit": "1Gi" } }
                    ]
                }
            }
        }
    });

    // The policy before the Job: a pod that starts before the rule that holds it in would have
    // a window with the namespace's default egress, whatever that is.
    kube.apply(&service_account)
        .await
        .map_err(|err| err.to_string())?;
    kube.apply(&network_policy)
        .await
        .map_err(|err| err.to_string())?;
    kube.apply(&job).await.map_err(|err| err.to_string())?;

    Ok(true)
}

/// Removes one run's three objects. Removing what is not there succeeds, so a cancellation of a
/// run that never got a pod is not an error.
pub async fn delete_workspace_job(kube: Option<&KubeClient>, namespace: &str, run_id: &str) {
    if let Some(kube) = kube {
        let name = format!("agent-run-{run_id}");
        let _ = kube.delete("batch/v1", "Job", namespace, &name).await;
        let _ = kube
            .delete("networking.k8s.io/v1", "NetworkPolicy", namespace, &name)
            .await;
        let _ = kube.delete("v1", "ServiceAccount", namespace, &name).await;
    }
}
