use std::path::MAIN_SEPARATOR;

use anyhow::{Context, Result};
use indexmap::{map::Entry, IndexMap};
use next_core::{
    app_structure::find_app_dir,
    mode::NextMode,
    next_client::{get_client_chunking_context, get_client_compile_time_info},
    next_config::NextConfig,
    next_server::{get_server_chunking_context, get_server_compile_time_info},
    util::NextSourceConfig,
};
use serde::{Deserialize, Serialize};
use turbo_tasks::{
    debug::ValueDebugFormat, trace::TraceRawVcs, unit, TaskInput, TransientValue, Vc,
};
use turbopack_binding::{
    turbo::{
        tasks_env::ProcessEnv,
        tasks_fs::{DiskFileSystem, FileSystem, FileSystemPath, VirtualFileSystem},
    },
    turbopack::{
        build::BuildChunkingContext,
        core::{
            chunk::ChunkingContext, compile_time_info::CompileTimeInfo, environment::ServerAddr,
            PROJECT_FILESYSTEM_NAME,
        },
        dev::DevChunkingContext,
        ecmascript::chunk::EcmascriptChunkingContext,
        env::dotenv::load_env,
        node::execution_context::ExecutionContext,
        turbopack::evaluate_context::node_build_environment,
    },
};

use crate::{
    app::{AppProject, OptionAppProject},
    entrypoints::Entrypoints,
    pages::PagesProject,
    route::{Endpoint, Route},
};

#[derive(Debug, Serialize, Deserialize, Clone, TaskInput)]
#[serde(rename_all = "camelCase")]
pub struct ProjectOptions {
    /// A root path from which all files must be nested under. Trying to access
    /// a file outside this root will fail. Think of this as a chroot.
    pub root_path: String,

    /// A path inside the root_path which contains the app/pages directories.
    pub project_path: String,

    /// The contents of next.config.js, serialized to JSON.
    pub next_config: String,

    /// Whether to watch the filesystem for file changes.
    pub watch: bool,

    /// An upper bound of memory that turbopack will attempt to stay under.
    pub memory_limit: Option<u64>,
}

#[derive(Serialize, Deserialize, TraceRawVcs, PartialEq, Eq, ValueDebugFormat)]
pub struct Middleware {
    pub endpoint: Vc<Box<dyn Endpoint>>,
    pub config: NextSourceConfig,
}

#[turbo_tasks::value]
pub struct Project {
    /// A root path from which all files must be nested under. Trying to access
    /// a file outside this root will fail. Think of this as a chroot.
    root_path: String,

    /// A path inside the root_path which contains the app/pages directories.
    project_path: String,

    /// Whether to watch the filesystem for file changes.
    watch: bool,

    /// Next config.
    next_config: Vc<NextConfig>,

    browserslist_query: String,

    mode: NextMode,
}

#[turbo_tasks::value_impl]
impl Project {
    #[turbo_tasks::function]
    pub async fn new(options: ProjectOptions) -> Result<Vc<Self>> {
        let next_config = NextConfig::from_string(options.next_config);
        Ok(Project {
            root_path: options.root_path,
            project_path: options.project_path,
            watch: options.watch,
            next_config,
            browserslist_query: "last 1 Chrome versions, last 1 Firefox versions, last 1 Safari \
                                 versions, last 1 Edge versions"
                .to_string(),
            mode: NextMode::Development,
        }
        .cell())
    }

    #[turbo_tasks::function]
    async fn app_project(self: Vc<Self>) -> Result<Vc<OptionAppProject>> {
        let this = self.await?;
        let app_dir = find_app_dir(self.project_path()).await?;

        Ok(Vc::cell(if let Some(app_dir) = &*app_dir {
            Some(AppProject::new(self, *app_dir, this.mode))
        } else {
            None
        }))
    }

    #[turbo_tasks::function]
    async fn pages_project(self: Vc<Self>) -> Result<Vc<PagesProject>> {
        let this = self.await?;
        Ok(PagesProject::new(self, this.mode))
    }

    #[turbo_tasks::function]
    async fn project_fs(self: Vc<Self>) -> Result<Vc<Box<dyn FileSystem>>> {
        let this = self.await?;
        let disk_fs = DiskFileSystem::new(
            PROJECT_FILESYSTEM_NAME.to_string(),
            this.root_path.to_string(),
        );
        if this.watch {
            disk_fs.await?.start_watching_with_invalidation_reason()?;
        }
        Ok(Vc::upcast(disk_fs))
    }

    #[turbo_tasks::function]
    async fn client_fs(self: Vc<Self>) -> Result<Vc<Box<dyn FileSystem>>> {
        let virtual_fs = VirtualFileSystem::new();
        Ok(Vc::upcast(virtual_fs))
    }

    #[turbo_tasks::function]
    async fn node_fs(self: Vc<Self>) -> Result<Vc<Box<dyn FileSystem>>> {
        let this = self.await?;
        let disk_fs = DiskFileSystem::new("node".to_string(), this.project_path.clone());
        disk_fs.await?.start_watching_with_invalidation_reason()?;
        Ok(Vc::upcast(disk_fs))
    }

    #[turbo_tasks::function]
    pub(super) fn node_root(self: Vc<Self>) -> Vc<FileSystemPath> {
        self.node_fs().root().join(".next".to_string())
    }

    #[turbo_tasks::function]
    pub(super) fn client_root(self: Vc<Self>) -> Vc<FileSystemPath> {
        self.client_fs().root()
    }

    #[turbo_tasks::function]
    fn project_root_path(self: Vc<Self>) -> Vc<FileSystemPath> {
        self.project_fs().root()
    }

    #[turbo_tasks::function]
    pub(super) fn client_relative_path(self: Vc<Self>) -> Vc<FileSystemPath> {
        self.client_root().join("_next".to_string())
    }

    #[turbo_tasks::function]
    pub(super) async fn project_path(self: Vc<Self>) -> Result<Vc<FileSystemPath>> {
        let this = self.await?;
        let root = self.project_root_path();
        let project_relative = this.project_path.strip_prefix(&this.root_path).unwrap();
        let project_relative = project_relative
            .strip_prefix(MAIN_SEPARATOR)
            .unwrap_or(project_relative)
            .replace(MAIN_SEPARATOR, "/");
        Ok(root.join(project_relative))
    }

    #[turbo_tasks::function]
    pub(super) fn env(self: Vc<Self>) -> Vc<Box<dyn ProcessEnv>> {
        load_env(self.project_path())
    }

    #[turbo_tasks::function]
    pub(super) async fn next_config(self: Vc<Self>) -> Result<Vc<NextConfig>> {
        Ok(self.await?.next_config)
    }

    #[turbo_tasks::function]
    pub(super) fn execution_context(self: Vc<Self>) -> Vc<ExecutionContext> {
        let node_root = self.node_root();

        let node_execution_chunking_context = Vc::upcast(
            DevChunkingContext::builder(
                self.project_path(),
                node_root,
                node_root.join("chunks".to_string()),
                node_root.join("assets".to_string()),
                node_build_environment(),
            )
            .build(),
        );

        ExecutionContext::new(
            self.project_path(),
            node_execution_chunking_context,
            self.env(),
        )
    }

    #[turbo_tasks::function]
    pub(super) fn client_compile_time_info(&self) -> Vc<CompileTimeInfo> {
        get_client_compile_time_info(self.mode, self.browserslist_query.clone())
    }

    #[turbo_tasks::function]
    pub(super) async fn server_compile_time_info(self: Vc<Self>) -> Result<Vc<CompileTimeInfo>> {
        let this = self.await?;
        Ok(get_server_compile_time_info(
            this.mode,
            self.env(),
            // TODO(alexkirsz) Fill this out.
            ServerAddr::empty(),
        ))
    }

    #[turbo_tasks::function]
    pub(super) async fn client_chunking_context(
        self: Vc<Self>,
    ) -> Result<Vc<Box<dyn EcmascriptChunkingContext>>> {
        let this = self.await?;
        Ok(get_client_chunking_context(
            self.project_path(),
            self.client_root(),
            self.client_compile_time_info().environment(),
            this.mode,
        ))
    }

    #[turbo_tasks::function]
    pub(super) fn server_chunking_context(self: Vc<Self>) -> Vc<BuildChunkingContext> {
        get_server_chunking_context(
            self.project_path(),
            self.node_root(),
            self.client_fs().root(),
            self.server_compile_time_info().environment(),
        )
    }

    #[turbo_tasks::function]
    pub(super) async fn ssr_chunking_context(self: Vc<Self>) -> Result<Vc<BuildChunkingContext>> {
        let ssr_chunking_context = self.server_chunking_context().with_layer("ssr".to_string());
        Vc::try_resolve_downcast_type::<BuildChunkingContext>(ssr_chunking_context)
            .await?
            .context("with_layer should not change the type of the chunking context")
    }

    #[turbo_tasks::function]
    pub(super) async fn ssr_data_chunking_context(
        self: Vc<Self>,
    ) -> Result<Vc<BuildChunkingContext>> {
        let ssr_chunking_context = self
            .server_chunking_context()
            .with_layer("ssr data".to_string());
        Vc::try_resolve_downcast_type::<BuildChunkingContext>(ssr_chunking_context)
            .await?
            .context("with_layer should not change the type of the chunking context")
    }

    #[turbo_tasks::function]
    pub(super) async fn rsc_chunking_context(self: Vc<Self>) -> Result<Vc<BuildChunkingContext>> {
        let rsc_chunking_context = self.server_chunking_context().with_layer("rsc".to_string());
        Vc::try_resolve_downcast_type::<BuildChunkingContext>(rsc_chunking_context)
            .await?
            .context("with_layer should not change the type of the chunking context")
    }

    /// Scans the app/pages directories for entry points files (matching the
    /// provided page_extensions).
    #[turbo_tasks::function]
    pub async fn entrypoints(self: Vc<Self>) -> Result<Vc<Entrypoints>> {
        let mut routes = IndexMap::new();
        let app_project = self.app_project();
        let pages_project = self.pages_project();

        if let Some(app_project) = &*app_project.await? {
            let app_routes = app_project.routes();
            routes.extend(app_routes.await?.iter().map(|(k, v)| (k.clone(), *v)));
        }

        for (pathname, page_route) in pages_project.routes().await?.iter() {
            match routes.entry(pathname.clone()) {
                Entry::Occupied(mut entry) => {
                    *entry.get_mut() = Route::Conflict;
                }
                Entry::Vacant(entry) => {
                    entry.insert(*page_route);
                }
            }
        }

        // TODO middleware
        Ok(Entrypoints {
            routes,
            middleware: None,
        }
        .cell())
    }

    /// Emits opaque HMR events whenever a change is detected in the chunk group
    /// internally known as `identifier`.
    #[turbo_tasks::function]
    pub fn hmr_events(self: Vc<Self>, _identifier: String, _sender: TransientValue<()>) -> Vc<()> {
        unit()
    }
}

#[turbo_tasks::function]
async fn project_fs(project_dir: String, watching: bool) -> Result<Vc<Box<dyn FileSystem>>> {
    let disk_fs = DiskFileSystem::new(PROJECT_FILESYSTEM_NAME.to_string(), project_dir.to_string());
    if watching {
        disk_fs.await?.start_watching_with_invalidation_reason()?;
    }
    Ok(Vc::upcast(disk_fs))
}
