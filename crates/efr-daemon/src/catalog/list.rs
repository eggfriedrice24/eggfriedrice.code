//! The model catalog of either provider kind, and one list of it as every reader sees
//! it.
//!
//! An OpenAI catalog always has a list: the table built into efr until a fetch or the
//! cache file gives another. An Anthropic catalog has no table, so it has no list until
//! a fetch or the cache file gives one. [`ModelList`] hides the difference from the
//! readers: the models, the default and where the list came from.

use efr_config::{ModelEntry, Settings};
use efr_protocol::{CatalogOrigin as WireOrigin, CatalogStatus};
use efr_provider::ModelInfo;
use jiff::Timestamp;

use crate::providers::{ANTHROPIC, API};

/// The cache file of the OpenAI catalogs, in the state root.
pub(crate) const CATALOG_FILE: &str = "model_catalog.json";

/// The cache file of the Anthropic catalog, in the state root. It has a name of its
/// own, so a change of `[model] provider` never reads the list of the other company.
pub(crate) const ANTHROPIC_CATALOG_FILE: &str = "anthropic_model_catalog.json";

/// The company whose models a provider serves, which decides the form of its catalog
/// and the config table that adds models to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Vendor {
    /// `openai-subscription` and `openai-api`, with `[openai] models`.
    OpenAi,
    /// `anthropic-api`, with `[anthropic] models`.
    Anthropic,
}

impl Vendor {
    /// The company of the provider `id`.
    pub(crate) fn of(id: &str) -> Vendor {
        if id == ANTHROPIC { Vendor::Anthropic } else { Vendor::OpenAi }
    }

    /// The key of the config's list of models for this company.
    pub(crate) fn models_key(self) -> &'static str {
        match self {
            Vendor::OpenAi => "openai.models",
            Vendor::Anthropic => "anthropic.models",
        }
    }

    /// The cache file of the catalog, in the state root.
    pub(crate) fn cache_file(self) -> &'static str {
        match self {
            Vendor::OpenAi => CATALOG_FILE,
            Vendor::Anthropic => ANTHROPIC_CATALOG_FILE,
        }
    }

    /// The entries of the config's list of models for this company.
    pub(crate) fn entries(self, settings: &Settings) -> &[ModelEntry] {
        let entries = match self {
            Vendor::OpenAi => settings.openai.models.as_deref(),
            Vendor::Anthropic => settings.anthropic.models.as_deref(),
        };
        entries.unwrap_or_default()
    }

    /// The model of a turn that names none when neither the catalog nor the config
    /// gives one: Claude Code's default for Anthropic, none for OpenAI, whose table is
    /// always there.
    pub(crate) fn fallback_model(self) -> Option<&'static str> {
        match self {
            Vendor::OpenAi => None,
            Vendor::Anthropic => Some(efr_provider_anthropic::DEFAULT_MODEL),
        }
    }
}

/// The shared catalog of the active provider: the provider reads it for each request,
/// the fetch task replaces its list.
#[derive(Debug, Clone)]
pub(crate) enum ProviderCatalog {
    /// The catalog of `openai-subscription` or `openai-api`.
    OpenAi(efr_provider_openai::ModelCatalog),
    /// The catalog of `anthropic-api`.
    Anthropic(efr_provider_anthropic::ModelCatalog),
}

impl ProviderCatalog {
    /// The current list.
    pub(crate) fn list(&self) -> ModelList {
        match self {
            ProviderCatalog::OpenAi(catalog) => ModelList::openai(&catalog.current()),
            ProviderCatalog::Anthropic(catalog) => {
                ModelList::anthropic(catalog.current().as_deref())
            }
        }
    }

    /// True when the catalog has a list. Only an Anthropic catalog can have none.
    pub(crate) fn has_list(&self) -> bool {
        match self {
            ProviderCatalog::OpenAi(_) => true,
            ProviderCatalog::Anthropic(catalog) => catalog.current().is_some(),
        }
    }

    /// The OpenAI catalog to share with an OpenAI provider of `backend`: this one, or a
    /// new one with the built-in table when this is the catalog of another company.
    /// efrd loads the catalog of `[model] provider`, so it never needs the new one.
    pub(crate) fn openai(
        &self,
        backend: efr_provider_openai::Backend,
    ) -> efr_provider_openai::ModelCatalog {
        match self {
            ProviderCatalog::OpenAi(catalog) => catalog.clone(),
            ProviderCatalog::Anthropic(_) => efr_provider_openai::ModelCatalog::new(
                efr_provider_openai::Catalog::builtin(backend),
            ),
        }
    }

    /// The Anthropic catalog to share with the Anthropic provider: this one, or a new
    /// one without a list when this is the catalog of another company.
    pub(crate) fn anthropic(&self) -> efr_provider_anthropic::ModelCatalog {
        match self {
            ProviderCatalog::Anthropic(catalog) => catalog.clone(),
            ProviderCatalog::OpenAi(_) => efr_provider_anthropic::ModelCatalog::new(),
        }
    }
}

/// One list of a catalog, whichever company sent it: what a turn, a prompt,
/// `models.list`, `admin.status` and the settings tool read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModelList {
    vendor: Vendor,
    models: Vec<ModelInfo>,
    default: Option<String>,
    origin: WireOrigin,
    fetched_at: Option<Timestamp>,
}

impl ModelList {
    /// The list of an OpenAI `catalog`.
    pub(crate) fn openai(catalog: &efr_provider_openai::Catalog) -> ModelList {
        let origin = match catalog.origin() {
            efr_provider_openai::CatalogOrigin::Backend => WireOrigin::Backend,
            efr_provider_openai::CatalogOrigin::Cache => WireOrigin::Cache,
            _ => WireOrigin::Builtin,
        };
        ModelList {
            vendor: Vendor::OpenAi,
            models: catalog.models(),
            default: catalog.default_model(),
            origin,
            fetched_at: catalog.fetched_at(),
        }
    }

    /// The list of an Anthropic `catalog`, or an empty one with the origin `missing`
    /// while there is none.
    pub(crate) fn anthropic(catalog: Option<&efr_provider_anthropic::Catalog>) -> ModelList {
        let Some(catalog) = catalog else {
            return ModelList {
                vendor: Vendor::Anthropic,
                models: Vec::new(),
                default: None,
                origin: WireOrigin::Missing,
                fetched_at: None,
            };
        };
        let origin = match catalog.origin() {
            efr_provider_anthropic::CatalogOrigin::Cache => WireOrigin::Cache,
            _ => WireOrigin::Backend,
        };
        ModelList {
            vendor: Vendor::Anthropic,
            models: catalog.models(),
            default: catalog.default_model(),
            origin,
            fetched_at: Some(catalog.fetched_at()),
        }
    }

    /// The company of the list.
    pub(crate) fn vendor(&self) -> Vendor {
        self.vendor
    }

    /// The models on offer, in the catalog's order.
    pub(crate) fn models(&self) -> &[ModelInfo] {
        &self.models
    }

    /// The catalog's model of a turn that names none.
    pub(crate) fn default_model(&self) -> Option<&str> {
        self.default.as_deref()
    }

    /// Where the list came from.
    pub(crate) fn origin(&self) -> WireOrigin {
        self.origin
    }

    /// When the backend last sent or confirmed the list.
    pub(crate) fn fetched_at(&self) -> Option<Timestamp> {
        self.fetched_at
    }

    /// Where the list of `provider` came from, on the wire.
    pub(crate) fn status(&self, provider: &str) -> CatalogStatus {
        CatalogStatus {
            provider: Some(provider.to_owned()),
            origin: self.origin(),
            fetched_at: self.fetched_at(),
        }
    }
}

/// The OpenAI backend of the provider `id`.
pub(crate) fn backend(id: &str) -> efr_provider_openai::Backend {
    if id == API {
        efr_provider_openai::Backend::Api
    } else {
        efr_provider_openai::Backend::Subscription
    }
}
