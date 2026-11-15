//! The model catalogs of all providers, and which providers have a login: the one model
//! list that `models.list` shows and that each turn checks its model against.
//!
//! The list holds the models of the provider of new conversations (`[model] provider`)
//! first, with the default model marked; then the models of each other provider with
//! a login, in the order of [`PROVIDERS`](crate::providers::PROVIDERS), never marked as
//! the default; then the models of each account that `[accounts]` names but that has
//! no login, from its last list, so a client can show them as signed out. Without a
//! login and a name in `[accounts]`, another provider's list is left out. Then the list
//! is grouped by kind (`efr_protocol::group_by_kind`): the provider of new
//! conversations leads its kind, then the default account of each kind, so a signed-out
//! account stands inside its kind; `admin.status` sends the accounts in the same order.
//! Each model names its provider, and the status of each catalog carries the account's
//! label, its login and the limits that it reached.
//!
//! The turns read the same list in the order of routing ([`Catalogs::routed`]): the
//! providers without a login move behind the others, so a model id that two providers
//! list, such as a GPT model of the ChatGPT plan and of an API key, goes to the first
//! one that can serve it.
//!
//! The logins are read from the credential store at start, again before each answer
//! of `models.list` and `admin.status` and after a reload
//! (`Providers::sync_logins`), and follow each login and logout of efrd, so the list
//! comes from memory and a turn never waits for it.
//!
//! When an account reaches a limit, [`Catalogs::account_out`] builds the cause of the
//! failed turn with the other accounts that serve its model: the switch offer.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use efr_config::Settings;
use efr_protocol::{
    AccountOut, AccountState, CatalogStatus, LimitReached, ModelInfo, OtherAccount, OthersFull,
};

use crate::catalog::Models;
use crate::providers::LimitBook;

/// The catalogs, each with its fetch, and the providers that have a login.
#[derive(Debug)]
pub(crate) struct Catalogs {
    /// The catalog of the provider of new conversations at start first; an account
    /// that a login adds while efrd runs comes last.
    all: RwLock<Vec<Arc<Models>>>,
    /// The first of `all`.
    first: Arc<Models>,
    logins: Mutex<BTreeSet<String>>,
    /// The limits that each account reached, which its model calls note.
    limits: Arc<LimitBook>,
}

/// The model list of the latest settings, with the status of each catalog in it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Listed {
    pub(crate) models: Vec<ModelInfo>,
    /// The catalog of the provider of new conversations first.
    pub(crate) catalogs: Vec<CatalogStatus>,
}

impl Catalogs {
    /// The catalog `first` of the provider of new conversations and the catalogs
    /// `others`, none of them with a login yet, with the limits of the accounts in
    /// `limits`.
    pub(crate) fn new(
        first: Arc<Models>,
        others: Vec<Arc<Models>>,
        limits: Arc<LimitBook>,
    ) -> Self {
        let all = std::iter::once(Arc::clone(&first)).chain(others).collect();
        Catalogs { all: RwLock::new(all), first, logins: Mutex::new(BTreeSet::new()), limits }
    }

    /// Every catalog, each with its own fetch.
    pub(crate) fn all(&self) -> Vec<Arc<Models>> {
        self.all.read().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Adds the catalog of an account that a login added while efrd runs.
    pub(crate) fn add(&self, models: Arc<Models>) {
        self.all.write().unwrap_or_else(PoisonError::into_inner).push(models);
    }

    /// The catalog of the provider of new conversations at start.
    pub(crate) fn first(&self) -> Arc<Models> {
        Arc::clone(&self.first)
    }

    /// The catalog of `provider`, if efrd keeps one.
    pub(crate) fn of(&self, provider: &str) -> Option<Arc<Models>> {
        self.all
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .find(|models| models.provider() == provider)
            .cloned()
    }

    /// The catalog of the provider of new conversations, `[model] provider` of
    /// `settings`. efrd does not start without it, so only a test that gives one
    /// catalog of another provider gets the first one.
    pub(crate) fn configured(&self, settings: &Settings) -> Arc<Models> {
        self.of(&settings.model.provider).unwrap_or_else(|| self.first())
    }

    /// Records whether `provider` has a login now.
    pub(crate) fn set_logged_in(&self, provider: &str, logged_in: bool) {
        let mut logins = self.logins.lock().unwrap_or_else(PoisonError::into_inner);
        if logged_in {
            logins.insert(provider.to_owned());
        } else {
            logins.remove(provider);
        }
    }

    /// True when `provider` has a login.
    pub(crate) fn logged_in(&self, provider: &str) -> bool {
        self.logins.lock().unwrap_or_else(PoisonError::into_inner).contains(provider)
    }

    /// The limits that each account reached.
    pub(crate) fn limits(&self) -> &Arc<LimitBook> {
        &self.limits
    }

    /// The catalogs of the model list of `settings`, in its order: the configured
    /// provider's first, then each other provider with a login, then each account that
    /// `[accounts]` names but that has no login; then grouped by kind
    /// ([`efr_protocol::group_by_kind`]), so a signed-out account stands inside its
    /// kind (TM-Accounts 8).
    fn shown(&self, settings: &Settings) -> Vec<Arc<Models>> {
        let configured = self.configured(settings);
        let all = self.all();
        let others = all.iter().filter(|other| !Arc::ptr_eq(other, &configured));
        let (logged_in, signed_out): (Vec<&Arc<Models>>, Vec<&Arc<Models>>) =
            others.partition(|other| self.logged_in(other.provider()));
        let named = signed_out
            .into_iter()
            .filter(|other| settings.accounts.label(other.provider()).is_some());
        let mut shown: Vec<Arc<Models>> = std::iter::once(Arc::clone(&configured))
            .chain(logged_in.into_iter().chain(named).map(Arc::clone))
            .collect();
        efr_protocol::group_by_kind(&mut shown, |models| models.provider(), configured.provider());
        shown
    }

    /// The providers of the model list of `settings`, in its order.
    pub(crate) fn order(&self, settings: &Settings) -> Vec<String> {
        self.shown(settings).iter().map(|models| models.provider().to_owned()).collect()
    }

    /// Where the list of `models` came from, with the account's label from
    /// `settings`, whether it has a login, and its limits.
    pub(crate) fn status_of(&self, models: &Models, settings: &Settings) -> CatalogStatus {
        let provider = models.provider();
        let mut status = models.status();
        status.label = settings.accounts.label(provider).map(str::to_owned);
        status.signed_out = !self.logged_in(provider);
        // NOTE: a signed-out account cannot run, so old limits would only mislead.
        status.limits = if status.signed_out { None } else { self.limits.limits(provider) };
        status
    }

    /// The model list of `settings`: the configured provider's models first, then
    /// those of each other provider with a login, then those of each account that
    /// `[accounts]` names but that has no login, from its last list.
    pub(crate) fn listed(&self, settings: &Settings) -> Listed {
        let mut models = Vec::new();
        let mut catalogs = Vec::new();
        for (at, shown) in self.shown(settings).iter().enumerate() {
            let mut listed = shown.effective(settings);
            if at > 0 {
                for model in &mut listed {
                    model.default = false;
                }
            }
            models.extend(listed);
            catalogs.push(self.status_of(shown, settings));
        }
        Listed { models, catalogs }
    }

    /// Why the account `provider` cannot serve a turn on `model`: it reached `reached`.
    /// The offer names the other accounts of the model list that list `model`, in its
    /// order: each one that is ready, each one that is signed out, and each one that is
    /// full when its reset comes before the one of `reached`, or `reached` has none. A
    /// signed-out account with no list at all, which never had a login, counts as
    /// listing `model` when it is of the kind of `provider`. A full account that the
    /// offer leaves out counts in `others_full`, with the soonest known reset of those
    /// accounts, so a client never says that no other account runs the model.
    /// efrd never switches by itself; the user picks one.
    pub(crate) fn account_out(
        &self,
        settings: &Settings,
        provider: &str,
        model: &str,
        reached: LimitReached,
    ) -> AccountOut {
        let label = |id: &str| settings.accounts.label(id).map(str::to_owned);
        let mut later: Option<OthersFull> = None;
        let others = self
            .shown(settings)
            .iter()
            .filter(|other| other.provider() != provider)
            .filter(|other| {
                let listed = other.effective(settings);
                // NOTE: an account that never had a login has no list to hold the
                // model; one of the same kind as the account that is out serves the
                // same models, so it gets its login line (TM-Accounts 5).
                listed.iter().any(|info| info.id == model)
                    || (listed.is_empty()
                        && !self.logged_in(other.provider())
                        && efr_config::provider_kind(other.provider())
                            == efr_config::provider_kind(provider))
            })
            .filter_map(|other| {
                let id = other.provider();
                let (state, resets_at) = match self.limits.reached(id) {
                    _ if !self.logged_in(id) => (AccountState::SignedOut, None),
                    None => (AccountState::Ready, None),
                    Some(full) => {
                        let sooner = match (full.resets_at, reached.resets_at) {
                            (Some(free), Some(stopped)) => free < stopped,
                            (Some(_), None) => true,
                            (None, _) => false,
                        };
                        if !sooner {
                            let full_later = later.get_or_insert(OthersFull { resets_at: None });
                            full_later.resets_at = match (full_later.resets_at, full.resets_at) {
                                (Some(soonest), Some(at)) => Some(soonest.min(at)),
                                (soonest, at) => soonest.or(at),
                            };
                            return None;
                        }
                        (AccountState::Full, full.resets_at)
                    }
                };
                Some(OtherAccount { provider: id.to_owned(), label: label(id), state, resets_at })
            })
            .collect();
        AccountOut {
            provider: provider.to_owned(),
            label: label(provider),
            model: model.to_owned(),
            reached,
            others,
            others_full: later,
        }
    }

    /// The list of [`listed`](Self::listed) in the order of routing: the models of the
    /// providers with a login first, each group in its own order. A turn goes to the
    /// first provider that lists its model.
    pub(crate) fn routed(&self, settings: &Settings) -> Vec<ModelInfo> {
        let (mut first, rest): (Vec<ModelInfo>, Vec<ModelInfo>) =
            self.listed(settings).models.into_iter().partition(|model| {
                model.provider.as_deref().is_some_and(|provider| self.logged_in(provider))
            });
        first.extend(rest);
        first
    }
}

#[cfg(test)]
mod tests;
