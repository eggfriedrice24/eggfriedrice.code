//! What tailscaled says about this node, and where the remote listener may listen.
//!
//! [`follow`] opens the IPN bus watch (mask 16386, never the obsolete bit 8) and turns
//! each state, each change of this node and the initial status into a [`View`]. A
//! `SelfChange` arrives with every full net map, also when nothing changed, so the
//! view goes out only when it differs. When the watch ends, tailscaled may only have
//! dropped a reader that fell behind: the follower asks the status, publishes it, and
//! opens the watch again after a pause. Only a status that fails too makes the view
//! [`View::Down`], which closes the listener and the phone sockets.

use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::time::Duration;

use efr_http::{BackendState, IpnNotify, LocalApi, TailscaleStatus, TailscaleVersion};
use efr_stdx::time::Clock;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

/// The first pause before the watch opens again.
const MIN_RETRY: Duration = Duration::from_secs(1);

/// The longest pause before the watch opens again.
const MAX_RETRY: Duration = Duration::from_secs(30);

/// What the remote listener knows of this node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum View {
    /// Nothing yet: the follower has not had an answer.
    Unknown,
    /// The listener may not listen, for this reason.
    Down(Down),
    /// The listener may listen on this node's tailnet address.
    Up(Tailnet),
}

/// Why the listener may not listen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Down {
    /// tailscaled did not answer.
    Unreachable,
    /// The backend does not run (stopped, logged out, starting).
    NotRunning,
    /// tailscaled runs in userspace networking mode: tailnet connections arrive on
    /// 127.0.0.1, so a listener on the tailnet address cannot exist.
    Userspace,
    /// tailscaled is older than 1.100, or did not say its version.
    OldVersion,
    /// The node has no IPv4 address in 100.64.0.0/10.
    NoAddress,
    /// The node or the tailnet has no stable id.
    NoIdentity,
}

impl Down {
    fn as_str(self) -> &'static str {
        match self {
            Down::Unreachable => "tailscaled does not answer",
            Down::NotRunning => "Tailscale does not run",
            Down::Userspace => "tailscaled runs in userspace networking mode",
            Down::OldVersion => "tailscaled is older than 1.100",
            Down::NoAddress => "this node has no Tailscale IPv4 address",
            Down::NoIdentity => "tailscaled names no node or tailnet id",
        }
    }
}

/// This node on its tailnet, as the status said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Tailnet {
    /// The address to listen on.
    pub(crate) ipv4: Ipv4Addr,
    /// Every tailnet address of this node. A peer at one of them is a process here.
    pub(crate) own_ips: Vec<IpAddr>,
    /// The stable id of this node.
    pub(crate) self_node: String,
    /// The user that owns this node; `None` for a tagged node, which belongs to no
    /// person, so no peer can be of the same user.
    pub(crate) self_user: Option<i64>,
    /// The stable id of the tailnet.
    pub(crate) tailnet: String,
    /// The MagicDNS name of this node, without the dot at its end.
    pub(crate) fqdn: Option<String>,
}

/// The view of `status`.
pub(crate) fn view_of(status: &TailscaleStatus) -> View {
    let version_ok = status
        .version
        .as_ref()
        .is_some_and(|version| version.at_least(TailscaleVersion::MINIMUM_REMOTE));
    if status.backend_state != BackendState::Running {
        return View::Down(Down::NotRunning);
    }
    if !status.tun {
        return View::Down(Down::Userspace);
    }
    if !version_ok {
        return View::Down(Down::OldVersion);
    }
    let Some(ipv4) = status.tailnet_ipv4() else {
        return View::Down(Down::NoAddress);
    };
    let (Some(self_node), Some(tailnet)) = (status.self_stable_id(), status.tailnet_stable_id())
    else {
        return View::Down(Down::NoIdentity);
    };
    let node = status.self_node.as_ref();
    let self_user = node.filter(|node| node.tags.is_empty()).map(|node| node.user_id);
    let fqdn = node
        .map(|node| node.dns_name.trim_end_matches('.').to_owned())
        .filter(|name| !name.is_empty());
    View::Up(Tailnet {
        ipv4,
        own_ips: status.tailscale_ips.clone(),
        self_node: self_node.to_owned(),
        self_user,
        tailnet: tailnet.to_owned(),
        fqdn,
    })
}

/// Follows tailscaled through `api` and sends each new view on `views` until `stop`.
pub(crate) async fn follow(
    api: LocalApi,
    clock: Arc<dyn Clock>,
    views: watch::Sender<View>,
    stop: CancellationToken,
) {
    let mut retry = MIN_RETRY;
    loop {
        let watched = tokio::select! {
            () = stop.cancelled() => return,
            watched = watch_once(&api, &views, &stop) => watched,
        };
        if stop.is_cancelled() {
            return;
        }
        if watched {
            retry = MIN_RETRY;
        }
        // NOTE: the end of the watch can be a reader that fell behind, so only a status
        // that fails too is a tailscaled that is gone.
        let view = tokio::select! {
            () = stop.cancelled() => return,
            status = api.status() => match status {
                Ok(status) => view_of(&status),
                Err(error) => {
                    tracing::debug!(error = %error, "tailscaled gave no status");
                    View::Down(Down::Unreachable)
                }
            },
        };
        publish(&views, view);
        tokio::select! {
            () = stop.cancelled() => return,
            () = clock.sleep(retry) => {}
        }
        retry = (retry * 2).min(MAX_RETRY);
    }
}

/// Reads one watch of the IPN bus to its end. True when it gave at least one message.
async fn watch_once(api: &LocalApi, views: &watch::Sender<View>, stop: &CancellationToken) -> bool {
    let mut watch = match api.watch().await {
        Ok(watch) => watch,
        Err(error) => {
            tracing::debug!(error = %error, "the watch of tailscaled did not open");
            return false;
        }
    };
    if let Some(version) = watch.version() {
        tracing::debug!(version = version.long(), "tailscaled's version");
    }
    let mut got = false;
    loop {
        let notify = tokio::select! {
            () = stop.cancelled() => return got,
            notify = watch.next() => notify,
        };
        let notify = match notify {
            Ok(Some(notify)) => notify,
            Ok(None) => return got,
            Err(error) => {
                tracing::debug!(error = %error, "the watch of tailscaled broke");
                return got;
            }
        };
        got = true;
        if notify.fell_behind() {
            tracing::debug!("the watch of tailscaled fell behind");
            return got;
        }
        if let Some(view) = view_of_notify(api, &notify).await {
            publish(views, view);
        }
    }
}

/// The view after `notify`: from its initial status, else from a new status when it
/// says that the state or this node changed; `None` when nothing that matters changed.
async fn view_of_notify(api: &LocalApi, notify: &IpnNotify) -> Option<View> {
    if let Some(status) = &notify.initial_status {
        return Some(view_of(status));
    }
    if notify.state.is_none() && notify.self_change.is_none() {
        return None;
    }
    Some(match api.status().await {
        Ok(status) => view_of(&status),
        Err(error) => {
            tracing::debug!(error = %error, "tailscaled gave no status");
            View::Down(Down::Unreachable)
        }
    })
}

/// Sends `view` when it differs from the last one, and logs the change.
fn publish(views: &watch::Sender<View>, view: View) {
    views.send_if_modified(|current| {
        if *current == view {
            return false;
        }
        match &view {
            View::Up(tailnet) => tracing::info!(
                address = %tailnet.ipv4,
                node = tailnet.self_node,
                tailnet = tailnet.tailnet,
                "Tailscale is up"
            ),
            View::Down(down) => tracing::warn!(reason = down.as_str(), "Tailscale is not usable"),
            View::Unknown => {}
        }
        *current = view;
        true
    });
}

#[cfg(test)]
mod tests;
