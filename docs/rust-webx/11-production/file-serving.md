# 文件服务的生产部署

[文件上传与下载](../05-request-pattern/file-upload-download.md)讲怎么写；本章讲怎么在生产环境跑。
涉及缓存、断点续传、反向代理、文件描述符与磁盘。

## 一、缓存与校验器

### 默认策略

框架只在响应**没有**设置 `Cache-Control` 时补默认值：

| 响应类型 | 默认 `Cache-Control` |
|----------|---------------------|
| 普通 API 响应 | `no-store` |
| 文件响应（`ResponseData::file` 系列） | `public, max-age=0, must-revalidate` |
| `wwwroot` 下的 `/assets/**` | `public, max-age=31536000, immutable` |
| `wwwroot` 下的其它静态文件 | `public, max-age=0, must-revalidate` |

文件响应默认选择「可缓存但必须重新验证」，是因为框架同时发出了 `ETag`——
配 `no-store` 的话客户端永远不会用它发条件请求，等于白算。

### 按内容类型调整

```rust
// 指纹化资源：内容变了 URL 就变，可以长期缓存
Ok(ResponseData::file(path)
    .header("cache-control", "public, max-age=31536000, immutable"))

// 含个人数据：只能私有缓存
Ok(ResponseData::file(report)
    .header("cache-control", "private, max-age=300"))

// 敏感单据：不落任何缓存
Ok(ResponseData::file(payslip)
    .header("cache-control", "no-store"))
```

### 校验器怎么选

| 方案 | 何时用 |
|------|--------|
| 自动推导（大小 + 纳秒 mtime） | 默认。零成本，文件一变就变 |
| `.entity_tag("\"sha256-…\"")` | 需要跨节点/跨重启逐字节一致，或回源对象存储带内容哈希 |
| `.last_modified(t)` | 校验器存在元数据里而不是文件系统里 |

> 自动推导的 `ETag` 是**强**校验器，因为断点续传需要强校验器（RFC 7233 §3.2）。
> 它不保证字节级唯一，只是「文件变了校验器就变」。要字节级保证就显式指定内容哈希。

多实例部署时注意：自动推导的 `ETag` 依赖文件的 mtime，多副本同步时若 mtime 不一致，
同一个文件在不同节点会给出不同 `ETag`，CDN 会缓存两份。这种情况下显式指定内容哈希。

### 内嵌资源的压缩与缓存

烤进二进制的静态资源可能以 brotli 存储，并按 `Accept-Encoding` 协商发送
（见 [OpenAPI 与 SPA 托管](openapi-spa.md#内嵌文件怎么压缩)）。运维上只需注意三点：

* **`Content-Encoding` 属于表示的一部分**：压缩表示与原文件是两个不同的实体，各自带
  自己的 `ETag`（`"sha256-…-br"` 与 `"sha256-…"`）。框架只对可能变化的响应发
  `Vary: Accept-Encoding`，缓存按此分桶即可，不会串味。
* **不要让代理二次压缩**：上游已经带了 `Content-Encoding`，规范的代理会跳过；若反向代理
  被配置成强制压缩，可能叠加或改写，务必让它按 `Content-Encoding` 与 `Vary` 正常处理。
* **不要剥掉 `Content-Encoding`**：一旦被中间层丢弃，客户端会拿到 brotli 字节却按明文解析。

内嵌资源同样支持 `Range` / `206`：`Content-Length` 与范围都基于**实际发送的表示**，
所以 `HEAD` 探到的长度与 `HEAD` 之后再 `Range` 的语义始终自洽。

## 二、断点续传与反向代理

客户端契约（`HEAD` 探长度与校验器 → `Range` + `If-Range` 续传）在
[文件上传与下载](../05-request-pattern/file-upload-download.md#25-断点续传)中说明。
反代要做的只是**别破坏它**。

### nginx

```nginx
location /files/ {
    proxy_pass         http://127.0.0.1:5000;
    proxy_http_version 1.1;

    # Range / If-Range / If-None-Match 默认就会透传，不要剥掉。
    # 但如果显式设置了下面这行，必须把 Range 加进去：
    proxy_set_header   Range $http_range;
    proxy_set_header   If-Range $http_if_range;

    # 关键：让上游的 Content-Length 原样送达客户端。
    # 打开 buffering 时 nginx 会先整体缓冲，1 GiB 的文件会先落 nginx 磁盘。
    proxy_buffering    off;

    # 未知长度的 chunked 响应本来就不该被缓冲。
    proxy_request_buffering off;   # 上传同理：边收边转发
}
```

要点：

* `proxy_buffering on`（默认）会把整个响应缓冲到 nginx 的临时文件，等于把流式下载
  变成两次磁盘写入。大文件下载务必关掉。
* `proxy_request_buffering on`（默认）会把上传**整体收完**再转发给上游，
  这会让框架的流式接收与大小限制失去意义（而且 nginx 默认 `client_max_body_size 1m`
  会先拒绝）。上传端点务必关掉，并调 `client_max_body_size`。
* 若前面还有 CDN，确认它转发 `Range` 并能缓存 `206`；很多 CDN 默认不缓存 `206`。

### Caddy

```
reverse_proxy 127.0.0.1:5000 {
    flush_interval -1      # 立即转发，不缓冲
}
```

### 用 nginx 直接发文件？

如果文件就在同一台机器上，让 nginx 用 `X-Accel-Redirect` 直接发会更省一次用户态拷贝：

```rust
Ok(ResponseData::bytes(Vec::new())
    .header("x-accel-redirect", "/internal-files/report.pdf")
    .header("content-type", "application/pdf")
    .download_name("报表.pdf"))
```

代价是**授权只在应用里做一次**、真正的字节由 nginx 直接发出去——如果文件是敏感的，
要么用 `internal` location（只允许内部重定向访问），要么就别这么做。
框架本身不做 `sendfile`（hyper 1.x 拿不到连接 IO），见下文。

## 三、文件描述符与并发

* **每个 Range 请求打开一个文件句柄**，响应结束后释放。`ulimit -n` 要按
  「并发下载数 + 余量」设置。多区间请求复用**同一个**句柄（逐段 `seek`），不会成倍占用。
* 每个连接还占一个 `max_connections` 名额（`App.MaxConnections`，默认 10000），
  与 fd 上限要一起看。
* 慢客户端会长期占住一个连接与一个 fd。用 nginx 的
  `proxy_read_timeout` / `send_timeout` 兜底。
* 多区间请求有 8 段上限，超出退回完整响应——既防放大攻击，也限制单请求的 `seek` 次数。

## 四、上传的运维要点

| 关注点 | 做法 |
|--------|------|
| 落盘目录 | `Form.TempDir` 指到**数据盘**，别用系统盘；目录必须可写 |
| 临时文件清理 | 框架在请求结束（`FormFile` 释放）时自动删除；进程被 `kill -9` 时可能残留，安排定期清理 |
| 磁盘配额 | `Form.MaxFileSize` × 并发上传数 是峰值占用上界，据此规划 |
| 单端点收紧 | 头像上传可以只要几 MiB，用 `app.useOptions` 调小；`Form` 是全局的，端点级校验在 Handler 里补 |
| 请求提前拒绝 | 声明超限的 `Content-Length` 在**读取上传内容之前**就返回 `413`，不消耗带宽与磁盘 |
| 反代 | `client_max_body_size`、`proxy_request_buffering off` |

Handler 里一定要在**请求结束前**调用 `save_as` / `copy_to` 把要保留的文件搬走——
临时文件随请求结构体释放而删除。

## 五、可观测性

`TimingMiddleware` 与访问日志能看到路径、状态码与耗时，但**看不到传输了多少字节**。
文件服务要补齐的指标：

* 每小时/每天的出流量，按端点与文件分组；
* `206` / `304` / `416` 的比例（`304` 高说明校验器有效，`206` 高说明续传在被使用）；
* `413` 次数（客户端在被拒之前已经发了多少？`Content-Length` 预检能让这个数字对应的
  带宽为 0）；
* 落盘目录的剩余空间与 inode。

## 六、上线检查清单

- [ ] `Form.TempDir` 指向数据盘且可写，有定期清理任务
- [ ] `Form.MaxFileSize` / `MaxRequestSize` 按业务收紧（默认 128 MiB / 256 MiB 是通用值）
- [ ] 反代关闭 `proxy_buffering` 与 `proxy_request_buffering`
- [ ] `ulimit -n` ≥ 预期并发下载数 × 2
- [ ] 敏感文件不在 `wwwroot` 下，走后端 `ResponseData::file` + `#[authorize]`
- [ ] 多实例部署时确认 `ETag` 策略一致（需要字节级一致就显式指定内容哈希）
- [ ] CDN 已确认能透传 `Range` 并缓存 `206`
- [ ] 反代/CDN 未二次压缩或剥离内嵌资源的 `Content-Encoding`，且按 `Vary: Accept-Encoding` 缓存
- [ ] 出流量、`206`/`304`/`413` 计数有监控

## 小结

框架把 HTTP 语义（校验器、Range、条件请求）做对了；生产环境要在它外面把
**代理缓冲、fd 上限、磁盘、可观测性**补齐。

上一节：[OpenAPI 与 SPA 托管](openapi-spa.md)
下一节：[优雅关闭与可观测性](graceful-shutdown.md)
