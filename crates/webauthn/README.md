# rust-webx-webauthn

WebAuthn/Passkey relying-party verification for rust-webx:

- registration (`none` attestation) and assertion verification;
- ES256 / EC2 P-256 credentials;
- `clientDataJSON` challenge/origin/type checks, `rpIdHash`, UP/UV flags;
- signature counter monotonicity;
- constant-time challenge comparison.

The crate is synchronous and network-free; the application owns challenges,
sessions, credential storage and browser JSON.

```rust
let config = WebAuthnConfig::new("example.com", "https://app.example.com")?
    .require_user_verification(true);
let challenge = WebAuthnChallenge::generate();
let registered = verify_registration(&config, &challenge, &client_data_json, &attestation_object, &raw_id)?;
let result = verify_assertion(&config, &registered.credential, &challenge, &client_data_json, &auth_data, &raw_id, &signature)?;
```
