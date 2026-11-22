# 支付迭代核验记录（无沙箱条件下的理论正确性）

> 范围：`rust-webx` 2.7.0 支付域（`crates/payment/*`，含前端支付方式目录）。
> 方法：官方文档 + 官方 SDK 源码对照 + 离线向量/请求构造测试 + 生产代码静态审计。
> 限制：本次迭代没有供应商真实沙箱/生产凭证，**不能**代替真实端到端验证；本文给出
> “理论正确、可复核”的证据与未关闭的发布阻塞项。

## 1. 核验基准

| 供应商 | 官方行为基准 | 本仓库对应实现 |
|---|---|---|
| Stripe | [API/SDKs](https://docs.stripe.com/sdks)、[Webhook 签名](https://docs.stripe.com/webhooks/signature)、官方 `stripe-ruby`/`stripe-python` 源码 | `crates/payment/stripe` |
| 支付宝 | [开放平台文档](https://opendocs.alipay.com/)、[`alipay-sdk-java`](https://github.com/alipay/alipay-sdk-java-all) | `crates/payment/alipay` |
| 微信支付 v3 | [APIv3 文档](https://pay.weixin.qq.com/doc/v3/merchant/4012062524)、[`wechatpay-apiv3`](https://github.com/wechatpay-apiv3)（java/go/php 官方 SDK） | `crates/payment/wechatpay` |
| PayPal | [REST API](https://developer.paypal.com/api/rest/)、[官方 server SDK 列表](https://developer.paypal.com/sdk/)、`paypal-server-sdks` | `crates/payment/paypal` |
| Adyen | [Checkout API](https://docs.adyen.com/api-explorer/)、[官方 SDK](https://github.com/Adyen) | `crates/payment/adyen` |
| Square | [Payments API](https://developer.squareup.com/reference/square/payments-api)、[官方 SDK](https://developer.squareup.com/docs/sdks) | `crates/payment/square` |

核验方式：

1. **算法/字节级**：签名串、规范化、编码、金额单位、回调验签按官方文档逐项比对；
2. **行为级**：请求/响应的字段、幂等键、状态映射按官方 SDK 源码与 API Explorer 示例比对；
3. **离线证据**：RSA/HMAC/AES-GCM 向量、请求构造快照、回调 fixture、MockUpstream 往返；
4. **静态审计**：生产代码不得出现 `todo!/unimplemented!/stub!/假数据`，`MockUpstream` 只在
   `#[cfg(test)]`/`test_support` 中使用；不支持的能力返回 `PaymentError::Unsupported` 并
   在手册记录。

## 2. 逐供应商核验

### Stripe

- 请求：PaymentIntents/Checkout 使用 form-encoded v1；金额为最小货币单位整数；
  `Idempotency-Key` 由 `order_id`/显式 key 生成。
- 回调：`Stripe-Signature` 的 `t` + `.` + raw body HMAC-SHA256，300 秒时间窗，多个 `v1`
  逐个常量时间比较；fixture 覆盖篡改 body/过期时间/轮换密钥。
- 查询：`pi_`/`cs_` 直查；商户订单号走 PaymentIntents Search（文档已声明索引延迟）。
- 退款：先解析 PaymentIntent，再按 `re_` 查退款；退款查询要求 provider refund id。
- 前端目录：Card + publishable key（从 `StripeAccountOptions::publishable_key` 读取）；
  不含 secret key。

### 支付宝

- 签名：RSA2（SHA256withRSA），参数排序/编码按开放平台规则；响应验签取原始
  `xxx_response` JSON 子串；通知验签排除 `sign`/`sign_type`。
- 证书：`app_cert_sn`/`alipay_root_cert_sn` 使用官方 SDK 的
  `MD5(issuer + 十进制 serial)` 算法（不是简化的 `MD5(DER)`），与官方 Java SDK 实现对齐。
- 幂等：`out_trade_no` 为商户订单锚点；退款 `out_request_no` 支持部分退款。
- 回调：异步通知按 raw form 校验；ACK 为字面量 `success`。
- 前端目录：page/wap/app/qr 四种渠道 + 公开 `app_id`。

### 微信支付 v3

- 请求签名：`METHOD\nURL\nTIMESTAMP\nNONCE\nBODY\n` 的 SHA256withRSA，`Authorization:
  WECHATPAY2-SHA256-RSA2048`；URL 使用 path+query，不含 host。
- 应答/回调：`timestamp\nnonce\nbody\n` 签名验证，支持平台证书与微信支付公钥两种模式；
  平台证书按 serial 缓存，未知 serial 触发刷新（官方 bootstrap 下载按官方规定豁免应答验签）。
- 回调解密：`AEAD_AES_256_GCM`，AAD 为 `associated_data`，nonce 12 字节；使用官方
  NIST/微信测试向量。
- 金额：分（整数）；仅 CNY（能力位声明）。
- 前端目录：native/jsapi/app/h5 + 公开 `app_id`；不输出 APIv3 key/私钥。

### PayPal

- OAuth2：client_credentials，token 缓存带过期提前量，并发刷新单飞（单测覆盖）。
- Orders v2：`intent=CAPTURE/AUTHORIZE`，`PayPal-Request-Id` 幂等；capture/void/refund
  以 PayPal 对象 id 为键；部分捕获只在 AUTHORIZE 手动捕获下支持。
- 回调：`POST /v1/notifications/verify-webhook-signature`，校验
  `PAYPAL-TRANSMISSION-*` + `webhook_id`；这是官方推荐方式（非本地 HMAC）。
- 前端目录：PayPal + 公开 `client_id` + 环境；不输出 client_secret。
- 限制：Orders v2 没有按商户订单号反查的公开 API；文档与设计均已声明。

### Adyen

- Checkout：`/payments`、`/payments/{pspReference}/captures|cancels|refunds`，
  `Idempotency-Key`，金额最小单位整数，`merchantAccount` 必填。
- 回调 HMAC：`pspReference:originalReference:merchantAccountCode:merchantReference:
  amount.value:amount.currency:eventCode:success`，反斜杠/冒号按官方规则转义；
  支持 Base64（官方）和十六进制（兼容旧配置）；`[accepted]` ACK。
- 查询：Checkout API 没有公开的支付状态 GET；通用 `get_payment` 返回 `Unsupported`，
  提供 `get_payment_with_details`（需要 redirectResult/threeDSResult）和批量 `verify_all`。
- 前端目录：card/applepay/googlepay/paypal + client_key/merchantAccount/environment；
  不输出 API key/HMAC key。

### Square

- 请求：`POST /v2/payments`、`complete`、`cancel`、`/v2/refunds`，金额最小单位；
  `Idempotency-Key` 在 JSON body；`Square-Version` 固定。
- 回调：`x-square-hmacsha256-signature` = Base64(HMAC-SHA256(signature_key,
  notification_url + raw_body))，常量时间比较。
- 查询/退款以 Square payment/refund id 为键；下单需要客户端 token
  （metadata `square_source_id`）；没有商户参考号反查 API。
- 前端目录：Card（Web Payments SDK）+ application_id/location_id/environment；
  不输出 access token/signature key。

## 3. 全流程理论走通

1. `Host::builder().add_<provider>().build()` → 自动注册 `Payments`、provider factory、
   builder 与静态健康检查；
2. 前端调 `Payments::payment_options_for(&user, Some(&money))` 得到目录；
3. 前端选定 `provider/account/method/flow`，后端用 `PaymentScope` 调 `create`；
4. 返回 `ClientAction`：`ClientSdk`/`Redirect`/`QrCode`/`App`；
5. 供应商回调 → `payment_webhook!` 路由 → `verify_webhook`（原始 body 验签/解密）→
   inbox（可选）→ `ack`；
6. 业务 Handler 调 `get/capture/cancel/refund/get_refund`，错误按 `PaymentError`
   映射 400/404/409/500/502/503；
7. 配置变更：自定义工厂更新 DB → `payments.invalidate(scope)`（或 TTL）→ 后续请求与目录
   立即使用新配置。

该链路在离线测试中已覆盖：核心 e2e（Host + raw webhook + ack）、六个适配器的请求构造/
签名/回调 fixture、inbox 去重/退避/死信、catalog 公共字段与密钥泄漏断言。

## 4. 静态审计：空桩/模拟/绕过

- 生产代码中没有 `todo!`、`unimplemented!`、`stub!`、假数据分支；`panic!`/`unreachable!`
  仅出现在 `#[cfg(test)]` 测试断言和不可能到达的测试辅助分支。
- `MockUpstream`/`test_support` 只被测试使用；生产路径只使用 `rust-webx-client::HttpClient`。
- 不支持的能力显式返回 `PaymentError::Unsupported`（如 Adyen 通用查询、PayPal/Square 的
  商户号反查），不是静默空实现；手册 §14 已逐项记录。
- 回调验签失败返回 400；配置源不可用返回 503；未验签数据不会进入业务。

## 5. 未关闭的发布阻塞项

1. **真实沙箱**：Stripe test、支付宝沙箱、PayPal sandbox、Adyen test、Square sandbox
   需要各跑一次真实下单/回调/退款；微信 v3 用官方验收用例 + 回放。无凭证时本文只能保证
   理论正确与离线证据。
2. **目录方法真实性**：静态配置默认列出适配器支持的方法；需要按账户启用/禁用方法时，
   由自定义工厂的 `payment_options` 过滤（后续可加账户级配置）。
3. **运行时工厂 readiness**：自定义 `IPaymentServiceFactory` 的 `probe()` 需要应用接入
   健康检查；默认静态 provider 已有 `payments.<provider>` 检查。
4. **真实回调公网可达性**：需要部署到可被供应商访问的 HTTPS 域名后验证签名/重投。
5. **供应商 API 版本漂移**：按 2.7.0 基线固定；后续用 nightly 沙箱/契约回放跟踪。

## 6. 关联文档

- [支付手册](../rust-webx/13-extensibility/payments.md)：使用方式、目录、发布检查清单。
- [支付域设计](payments.md)：架构取舍与供应商差异。
- [运行时工厂评审](payment-runtime-factory.md)：DI/工厂/缓存/失效决策。
- [CHANGELOG](../../CHANGELOG.md)：2.7.0 交付内容。
