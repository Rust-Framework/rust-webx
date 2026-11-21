use webx::*;

struct NotARequest;

#[get("/wrong")]
impl Clone for NotARequest {
    fn clone(&self) -> Self {
        NotARequest
    }
}

fn main() {}
