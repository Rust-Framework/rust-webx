# 请求体流式读取

普通 DTO 绑定会把整个 body 缓冲成 JSON；大上传/代理/透传端点需要边收边处理时，用
`IHttpRequest::body_stream()`。

```rust
use futures_util::StreamExt;
use std::ops::ControlFlow;
use webx::*;

struct BodyLength;

#[async_trait::async_trait]
impl IMiddleware for BodyLength {
    async fn invoke(&self, ctx: &mut dyn IHttpContext) -> Result<ControlFlow<()>> {
        let mut stream = ctx.request_mut().body_stream().await?;
        let mut length = 0usize;
        while let Some(chunk) = stream.next().await {
            length += chunk?.len();
        }
        ctx.response_mut().set_header("x-body-length", &length.to_string());
        Ok(ControlFlow::Continue(()))
    }
}
```

`body_stream()` 返回 `Pin<Box<dyn Stream<Item = Result<Bytes>> + Send>>`。

## 语义

- 默认实现回退 `body_bytes`，作为一个 chunk 返回；测试/替代 host 无需改。
- host 覆盖为实时 hyper `Incoming` 流；body 只能消费一次，之后 `body_bytes` 报“已消费”。
- 流式读取仍按 `App.MaxBodySize` 计数；超出返回 **413**。
- 与 `multipart()` 互斥：不能同时以表单和裸流读取。
- 普通 Handler 只拿 DTO，拿不到 `IHttpContext`；裸流场景应使用 middleware 或自定义
  `IRequest<Response>` 适配层。

## 按路由覆盖请求体上限

全局 `App.MaxBodySize` 对普通 JSON 足够；文件上传/批量端点可以用属性单独放宽或收紧：

```rust
#[post("/v1/files")]
#[max_body_size(64 * 1024 * 1024)] // 64 MiB
impl IRequest<FileInfo> for UploadRequest {}
```

- 在**绑定前**应用，超过上限直接 `413`（不会先读进内存再判断）。
- 只影响非 multipart body；multipart 仍由 `Form.MaxRequestSize`/`Form.MaxFileSize` 约束。
- 未声明属性的路由沿用全局上限；`#[max_body_size(0)]` 是编译错误。
- OpenAPI/路由元数据不受影响，这是传输层限制。

## 出站流式 body

`rust-webx-client` 的 `OutboundRequest::body_stream` 接受 chunk 流，逐块发送：

```rust
use bytes::Bytes;
use futures_util::stream;
use webx_client::{HttpClientError, OutboundRequest};

let chunks = stream::iter(vec![
    Ok::<Bytes, HttpClientError>(Bytes::from_static(b"hello ")),
    Ok(Bytes::from_static(b"stream")),
]);

let request = OutboundRequest::post("/upload").body_stream(chunks);
```

- 内部用 `UnsyncBoxBody` + `StreamBody` 转成 hyper body；
- `body(bytes)` 仍走零拷贝 `Full<Bytes>` 快路径；
- 因为可能持有流，`OutboundRequest` **不再 `Clone`**（仍是 `Debug`）；
- 失败/取消由流本身结束；首字节前的重试策略仍由调用方决定。

## 适用场景

- 上游请求透传/代理；
- 大 batch、audio、文件流；
- 边收边计算 hash/长度/校验，不落全量内存。

## 边界

- 已实现：入站 trait/host 实时流/413、middleware 端到端测试；出站 `body_stream` + 真实 hyper 测试；
  按路由 `#[max_body_size(..)]` 覆盖非 multipart 上限。
- 未实现：出站 content-length/chunked 策略、背压调优、multipart 的按路由预算。
- 不要对普通 JSON DTO 同时调用 `body_stream` 和绑定；绑定会先消费 body。
- 流式读取后不要再调用 `multipart()`/`body_bytes()`。
- `OutboundRequest` 不再 `Clone`；需要重试时重建请求或缓存 body 字节。
