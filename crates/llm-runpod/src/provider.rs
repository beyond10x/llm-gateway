//! Runpod as an `llm_provision::HostingProvider`.
//!
//! Ported from `RunpodApi` and `PodManager::start` in llmgw `src/runpod.rs`: ordered GPU
//! fallback, readiness probing and crash-loop detection by decreasing container uptime. What
//! changed is identity. llmgw adopted any pod whose name matched; here a pod is a resource key
//! whose incarnation is the Runpod pod id, and whose owner, epoch and request id are read back
//! from the environment it was created with. The hosting contract decides adoption from those.

use std::{
    collections::{BTreeMap, VecDeque},
    sync::Arc,
};

use llm_provision::{
    CreateOutcome, CreateRequest, Dispatch, HostingProvider, Identifier, Inventory,
    ObservedResource, ProviderState, ResourceKey, StopOutcome,
};

use crate::{
    Clock, CreateAnswer, Pod, PodStatus, Probe, RunpodModel, RunpodTransport, TAG_EPOCH, TAG_OWNER,
    TAG_REQUEST, TerminateAnswer,
    request::{Tags, in_namespace, pod_request},
};

/// Why a pod can no longer serve and has to be replaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unserviceable {
    /// The container restarted at least the declared number of times inside the declared
    /// window while being polled.
    CrashLoop,
    /// The pod refused the vLLM key.
    CredentialRefused,
    /// Runpod reports the pod exited. It still exists, still holds its disk and is still
    /// billed, so it is not stopped until it is terminated.
    Exited,
}

#[derive(Debug, Default)]
struct Health {
    last_uptime: Option<u64>,
    /// When each observed restart was seen, oldest first.
    restarts: VecDeque<u64>,
    refused: bool,
    exited: bool,
}

/// A hosting provider backed by a Runpod transport.
pub struct RunpodProvider<T> {
    transport: T,
    provider: Identifier,
    account: Identifier,
    models: BTreeMap<Identifier, RunpodModel>,
    health: BTreeMap<String, Health>,
    clock: Arc<dyn Clock>,
}

impl<T: std::fmt::Debug> std::fmt::Debug for RunpodProvider<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RunpodProvider")
            .field("transport", &self.transport)
            .field("provider", &self.provider)
            .field("account", &self.account)
            .finish_non_exhaustive()
    }
}

impl<T: RunpodTransport> RunpodProvider<T> {
    /// A provider over one Runpod account, for the declared models keyed by alias.
    ///
    /// `provider` and `account` are the scope the resource keys are written in; they must match
    /// the controller's `HostingPolicy`, which refuses any key outside it. The clock timestamps
    /// observed restarts, so that only those inside a model's crash window count.
    pub fn new(
        transport: T,
        provider: Identifier,
        account: Identifier,
        models: BTreeMap<Identifier, RunpodModel>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            transport,
            provider,
            account,
            models,
            health: BTreeMap::new(),
            clock,
        }
    }

    fn model_of(&self, key: &ResourceKey) -> Option<&RunpodModel> {
        let alias = key
            .name
            .as_str()
            .strip_prefix(crate::POD_NAME_PREFIX)
            .and_then(|alias| Identifier::new(alias).ok())?;
        self.models.get(&alias)
    }

    /// Why this pod has to be replaced, if it does.
    pub fn unserviceable(&self, key: &ResourceKey) -> Option<Unserviceable> {
        let health = self.health.get(key.incarnation.as_str())?;
        if health.exited {
            return Some(Unserviceable::Exited);
        }
        if health.refused {
            return Some(Unserviceable::CredentialRefused);
        }
        let model = self.model_of(key)?;
        // `poll` keeps only restarts inside the window, so what is left is what counts.
        (health.restarts.len() >= model.crash_restart_limit as usize)
            .then_some(Unserviceable::CrashLoop)
    }

    fn key(&self, pod: &Pod) -> Option<ResourceKey> {
        Some(ResourceKey {
            provider: self.provider.clone(),
            account: self.account.clone(),
            name: Identifier::new(pod.name.clone()).ok()?,
            incarnation: Identifier::new(pod.id.clone()).ok()?,
        })
    }

    /// Describes a pod from what the control plane and, when asked, the pod itself answered.
    fn describe(&self, pod: &Pod, probe: Option<&Probe>) -> Option<ObservedResource> {
        let key = self.key(pod)?;
        // Only `Terminated` ends a resource. An exited pod still exists and is still billed, so
        // it is reported as present-but-not-running and retired through `unserviceable`.
        let state = match pod.status {
            PodStatus::Created | PodStatus::Exited => ProviderState::Pending,
            PodStatus::Running => ProviderState::Running,
            PodStatus::Terminated => ProviderState::Terminated,
        };
        let (ready, served_model) = match probe {
            Some(Probe::Ready { served_models }) => (
                Some(true),
                match served_models.as_slice() {
                    [one] => Identifier::new(one.clone()).ok(),
                    _ => None,
                },
            ),
            Some(Probe::NotReady | Probe::Refused) => (Some(false), None),
            Some(Probe::Unreachable) | None => (None, None),
        };
        Some(ObservedResource {
            // The proxy address exists only for a running pod; any other status reports none.
            endpoint: (pod.status == PodStatus::Running)
                .then(|| format!("https://{}-8000.proxy.runpod.net", pod.id)),
            key,
            state,
            ready,
            served_model,
            owner: pod
                .env
                .get(TAG_OWNER)
                .and_then(|owner| Identifier::new(owner.clone()).ok()),
            epoch: pod.env.get(TAG_EPOCH).and_then(|epoch| epoch.parse().ok()),
            request_id: pod
                .env
                .get(TAG_REQUEST)
                .and_then(|request| Identifier::new(request.clone()).ok()),
        })
    }

    /// Polls one running pod: readiness, and the uptime that reveals a restart.
    fn poll(&mut self, pod: &Pod) -> Probe {
        let probe = self.transport.probe_ready(&pod.id);
        let uptime = self.transport.container_uptime(&pod.id);
        let now_ms = self.clock.now_ms();
        let window = self
            .key(pod)
            .and_then(|key| self.model_of(&key).map(|model| model.crash_window_ms));
        let health = self.health.entry(pod.id.clone()).or_default();
        if let (Some(now), Some(before)) = (uptime, health.last_uptime)
            && now < before
        {
            health.restarts.push_back(now_ms);
        }
        // Restarts older than the window no longer count, and are not kept: a long-lived pod
        // holds at most the restarts of one window. The window's end is inclusive. A pod of no
        // declared model keeps none, because nothing could ever count them.
        prune_restarts(&mut health.restarts, now_ms, window.unwrap_or(0));
        if uptime.is_some() {
            health.last_uptime = uptime;
        }
        if probe == Probe::Refused {
            health.refused = true;
        }
        probe
    }
}

/// Drops every restart more than `window_ms` before `now_ms`.
fn prune_restarts(restarts: &mut VecDeque<u64>, now_ms: u64, window_ms: u64) {
    while restarts
        .front()
        .is_some_and(|&at| now_ms.saturating_sub(at) > window_ms)
    {
        restarts.pop_front();
    }
}

impl<T: RunpodTransport> HostingProvider for RunpodProvider<T> {
    fn create(&mut self, request: &CreateRequest) -> CreateOutcome {
        let not_sent = CreateOutcome {
            dispatch: Dispatch::NotSent,
            resource: None,
        };
        // An undeclared model, or a specification whose image is not the declared one, is
        // never sent: this adapter does not silently substitute either.
        let Some(model) = self.models.get(&request.spec.model) else {
            return not_sent;
        };
        if model.image != request.spec.image {
            return not_sent;
        }
        let tags = Tags {
            owner: request.owner.as_str(),
            epoch: request.epoch,
            request_id: request.request_id.as_str(),
        };
        for gpu in &model.gpu_types {
            let body = pod_request(
                request.spec.resource_name.as_str(),
                request.spec.model.as_str(),
                gpu,
                model,
                &tags,
            );
            match self.transport.create_pod(&body) {
                CreateAnswer::Created(pod) => {
                    return match self.describe(&pod, None) {
                        Some(resource) => CreateOutcome {
                            dispatch: Dispatch::Accepted,
                            resource: Some(resource),
                        },
                        // Created, but with an identity this contract cannot hold: as
                        // unaccountable as a lost answer.
                        None => CreateOutcome {
                            dispatch: Dispatch::Unknown,
                            resource: None,
                        },
                    };
                }
                // Placement fails per host, so the next declared GPU is tried.
                CreateAnswer::Refused => {}
                // A lost answer may have allocated. Trying another GPU could pay twice.
                CreateAnswer::Lost => {
                    return CreateOutcome {
                        dispatch: Dispatch::Unknown,
                        resource: None,
                    };
                }
            }
        }
        CreateOutcome {
            dispatch: Dispatch::Rejected,
            resource: None,
        }
    }

    fn stop(&mut self, key: &ResourceKey, _epoch: u64) -> StopOutcome {
        // Runpod has no conditional delete, so the epoch cannot travel; the controller fences
        // before this is reached.
        let dispatch = match self.transport.terminate_pod(key.incarnation.as_str()) {
            TerminateAnswer::Terminated => {
                self.health.remove(key.incarnation.as_str());
                return StopOutcome {
                    dispatch: Dispatch::Accepted,
                    evidence: Identifier::new(format!("runpod-terminated-{}", key.incarnation))
                        .ok(),
                };
            }
            TerminateAnswer::Refused => Dispatch::Rejected,
            TerminateAnswer::Lost => Dispatch::Unknown,
        };
        StopOutcome {
            dispatch,
            evidence: None,
        }
    }

    fn inventory(&mut self) -> Inventory {
        let Ok(listing) = self.transport.list_pods() else {
            return Inventory {
                completeness: llm_provision::Completeness::Partial,
                resources: Vec::new(),
            };
        };
        let mut complete = listing.complete;
        let mut resources = Vec::new();
        for pod in listing.pods.iter().filter(|pod| in_namespace(&pod.name)) {
            if pod.status == PodStatus::Exited {
                self.health.entry(pod.id.clone()).or_default().exited = true;
            }
            let probe = (pod.status == PodStatus::Running).then(|| self.poll(pod));
            match self.describe(pod, probe.as_ref()) {
                Some(resource) => resources.push(resource),
                // A pod in this namespace that cannot be described was still listed; leaving
                // it out of a "complete" answer would read as its absence.
                None => complete = false,
            }
        }
        Inventory {
            completeness: if complete {
                llm_provision::Completeness::Complete
            } else {
                llm_provision::Completeness::Partial
            },
            resources,
        }
    }

    /// Runpod's create call takes no idempotency key.
    fn honours_idempotency_key(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::prune_restarts;

    #[test]
    fn restarts_outside_the_window_are_dropped_and_its_end_is_kept() {
        let mut restarts: VecDeque<u64> = [100, 400, 500, 900].into_iter().collect();
        prune_restarts(&mut restarts, 1_000, 500);
        assert_eq!(
            restarts,
            VecDeque::from([500, 900]),
            "1 000 - 500 is inside"
        );
        prune_restarts(&mut restarts, 10_000, 500);
        assert!(restarts.is_empty());
    }
}
