use anyhow::Result;
use clap::{Args, CommandFactory};
use clap_complete::{generate, Shell};
use std::io;

use super::Cli;

#[derive(Args, Debug)]
pub struct CompletionsCommand {
    /// The shell to generate completions for
    pub shell: Shell,
}

pub fn execute(cmd: CompletionsCommand) -> Result<()> {
    let mut app = Cli::command();
    generate(cmd.shell, &mut app, "squads-cli", &mut io::stdout());
    Ok(())
}
