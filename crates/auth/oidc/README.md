# rust-webx-oidc

OpenID Connect **client** for rust-webx services:

- discovery (`/.well-known/openid-configuration`, issuer/HTTPS checks)
- authorization-code + PKCE (S256 required)
- id_token validation through the framework JWKS resolver (`iss`/`aud`/`nonce`/`exp`/`alg`/`kid`)
- session bridge: `OidcSession::to_token_claims()` feeds the framework `ITokenIssuer`

The crate deliberately does **not** implement an OpenID Provider or social-login
button catalog; it is the relying-party (RP) side.

```rust
let http = std::sync::Arc::new(HttpOidcHttp::new("https://id.example.com", Duration::from_secs(3))?);
let oidc = OidcClient::discover(http, "https://id.example.com", "travel-web", ClientAuth::None).await?;
let auth = oidc.authorization_url(AuthorizationRequest::new("https://app.example.com/cb").pkce_s256());
// store auth.state / auth.nonce / auth.pkce_verifier in the browser session
```
