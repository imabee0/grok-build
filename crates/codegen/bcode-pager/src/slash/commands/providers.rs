use crate::app::actions::Action;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand, slash_meta};

pub struct ProvidersCommand;

impl SlashCommand for ProvidersCommand {
    slash_meta! {
        name: "providers",
        description: "Manage model provider credentials and named accounts",
        usage: "/providers",
    }

    fn run(&self, _ctx: &mut CommandExecCtx, _args: &str) -> CommandResult {
        CommandResult::Action(Action::OpenProviderManager)
    }
}
