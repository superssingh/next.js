use anyhow::{bail, Result};
use turbo_tasks::Vc;
use turbopack_binding::turbopack::core::{
    asset::{Asset, AssetContent},
    chunk::{ChunkableModule, ChunkingContext},
    ident::AssetIdent,
    module::Module,
    output::OutputAssets,
    reference::AssetReferences,
};

/// A [`NextDynamicEntryModule`] is a marker asset used to indicate which
/// dynamic assets should appear in the dynamic manifest.
#[turbo_tasks::value(transparent)]
pub struct NextDynamicEntryModule {
    pub client_entry_module: Vc<Box<dyn Module>>,
}

#[turbo_tasks::value_impl]
impl NextDynamicEntryModule {
    /// Create a new [`NextDynamicEntryModule`] from the given source CSS
    /// asset.
    #[turbo_tasks::function]
    pub fn new(client_entry_module: Vc<Box<dyn Module>>) -> Vc<NextDynamicEntryModule> {
        NextDynamicEntryModule {
            client_entry_module,
        }
        .cell()
    }

    #[turbo_tasks::function]
    pub async fn client_chunks(
        self: Vc<Self>,
        client_chunking_context: Vc<Box<dyn ChunkingContext>>,
    ) -> Result<Vc<OutputAssets>> {
        let this = self.await?;

        let Some(client_entry_module) =
            Vc::try_resolve_sidecast::<Box<dyn ChunkableModule>>(this.client_entry_module).await?
        else {
            bail!("dynamic client asset must be chunkable");
        };

        let client_entry_chunk = client_entry_module.as_root_chunk(client_chunking_context);
        Ok(client_chunking_context.chunk_group(client_entry_chunk))
    }
}

#[turbo_tasks::function]
fn dynamic_modifier() -> Vc<String> {
    Vc::cell("dynamic".to_string())
}

#[turbo_tasks::value_impl]
impl Module for NextDynamicEntryModule {
    #[turbo_tasks::function]
    fn ident(&self) -> Vc<AssetIdent> {
        self.client_entry_module
            .ident()
            .with_modifier(dynamic_modifier())
    }
}

#[turbo_tasks::value_impl]
impl Asset for NextDynamicEntryModule {
    #[turbo_tasks::function]
    fn content(&self) -> Result<Vc<AssetContent>> {
        // The client reference asset only serves as a marker asset.
        bail!("NextDynamicEntryModule has no content")
    }

    #[turbo_tasks::function]
    fn references(self: Vc<Self>) -> Vc<AssetReferences> {
        AssetReferences::empty()
    }
}
