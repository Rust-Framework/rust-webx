//! In-process handlers are a first-class mediator use case; they must not be
//! rejected by the HTTP route diagnostics just because they have no route.

use webx::*;

struct Command;
struct CommandResult;

impl IRequest<CommandResult> for Command {}

#[derive(Default)]
struct CommandHandler;

#[handler(in_process)]
#[async_trait::async_trait]
impl IRequestHandler<Command, CommandResult> for CommandHandler {
    async fn handle(&mut self, _req: Command) -> ActionResult<CommandResult> {
        Ok(CommandResult)
    }
}

#[test]
fn host_build_accepts_an_in_process_handler() {
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .try_build()
        .expect("in-process handlers must not be treated as orphan routes");
    drop(host);
}
