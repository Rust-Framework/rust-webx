//! Route-less handlers are first-class `IMediator` handlers. HTTP-ness is
//! determined by the Request's route, not by an annotation on the handler, so
//! a non-`Serialize` response must compile and must not block Host startup.

use webx::*;

struct Command;
struct CommandResult;

impl IRequest<CommandResult> for Command {}

#[derive(Default)]
struct CommandHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<Command, CommandResult> for CommandHandler {
    async fn handle(&mut self, _req: Command) -> ActionResult<CommandResult> {
        Ok(CommandResult)
    }
}

#[test]
fn host_build_accepts_route_less_mediator_handlers() {
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .try_build()
        .expect("route-less mediator handlers must not block Host startup");
    drop(host);
}
