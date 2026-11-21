use webx::*;

struct NotAHandler;

#[handler]
impl Default for NotAHandler {
    fn default() -> Self {
        NotAHandler
    }
}

fn main() {}
