use anyhow::Context;
use clap::Parser;
use codex_core::config::Config;
use codex_utils_cli::CliConfigOverrides;

/// Inspect the native fail-closed Infinity Agent runtime boundary.
#[derive(Debug, Parser)]
pub struct InfinityAgentCommand {
    #[command(subcommand)]
    action: InfinityAgentSubcommand,
}

#[derive(Debug, clap::Subcommand)]
enum InfinityAgentSubcommand {
    /// Verify the effective boundary and emit its canonical JSON attestation.
    Attest,
}

impl InfinityAgentCommand {
    pub async fn run(self, config_overrides: CliConfigOverrides) -> anyhow::Result<()> {
        match self.action {
            InfinityAgentSubcommand::Attest => {
                let overrides = config_overrides
                    .parse_overrides()
                    .map_err(anyhow::Error::msg)?;
                let config = Config::load_with_cli_overrides(overrides)
                    .await
                    .context("failed to load the Infinity Agent configuration")?;
                let attestation = config
                    .infinity_agent_safety_attestation()
                    .context("Infinity Agent safety attestation failed closed")?;
                println!(
                    "{}",
                    serde_json::to_string(&attestation)
                        .context("failed to serialize the Infinity Agent attestation")?
                );
            }
        }
        Ok(())
    }
}
