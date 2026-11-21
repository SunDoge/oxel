use zed_extension_api::{self as zed, settings::LspSettings};

const SERVER_ID: &str = "oxel-lsp";

struct OxelExtension;

impl zed::Extension for OxelExtension {
    fn new() -> Self {
        Self
    }

    fn language_server_command(
        &mut self,
        _: &zed::LanguageServerId,
        worktree: &zed::Worktree,
    ) -> zed::Result<zed::Command> {
        let binary = LspSettings::for_worktree(SERVER_ID, worktree)
            .ok()
            .and_then(|settings| settings.binary);
        let command = binary
            .as_ref()
            .and_then(|binary| binary.path.clone())
            .or_else(|| worktree.which("oxel"))
            .ok_or(
                "Could not find oxel. Install it on PATH or set lsp.oxel-lsp.binary.path in Zed.",
            )?;
        let args = binary
            .and_then(|binary| binary.arguments)
            .unwrap_or_else(|| {
                let mut args = vec!["lsp".to_owned()];
                if worktree.read_text_file("oxel.toml").is_ok() {
                    args.extend(["--manifest-path", "oxel.toml", "--tests"].map(str::to_owned));
                }
                args
            });
        Ok(zed::Command {
            command,
            args,
            env: worktree.shell_env(),
        })
    }

    fn dap_request_kind(
        &mut self,
        _: String,
        config: zed::serde_json::Value,
    ) -> zed::Result<zed::StartDebuggingRequestArgumentsRequest> {
        if config["request"].as_str() == Some("launch") {
            Ok(zed::StartDebuggingRequestArgumentsRequest::Launch)
        } else {
            Err("Oxel debug configurations use request = launch".to_owned())
        }
    }

    fn get_dap_binary(
        &mut self,
        adapter: String,
        task: zed::DebugTaskDefinition,
        user_path: Option<String>,
        worktree: &zed::Worktree,
    ) -> zed::Result<zed::DebugAdapterBinary> {
        let config = zed::serde_json::from_str(&task.config).map_err(|error| error.to_string())?;
        let request = self.dap_request_kind(adapter, config)?;
        let command = user_path
            .or_else(|| worktree.which("netcoredbg"))
            .ok_or("Install netcoredbg on PATH to debug Oxel programs.")?;
        Ok(zed::DebugAdapterBinary {
            command: Some(command),
            arguments: vec!["--interpreter=vscode".to_owned()],
            envs: worktree.shell_env(),
            cwd: None,
            connection: None,
            request_args: zed::StartDebuggingRequestArguments {
                configuration: task.config,
                request,
            },
        })
    }
}

zed::register_extension!(OxelExtension);
