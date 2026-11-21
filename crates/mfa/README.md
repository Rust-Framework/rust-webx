# rust-webx-mfa

Multi-factor primitives for rust-webx:

- **TOTP** (RFC 6238): SHA-1/256/512, 6–8 digits, ±skew, replay protection via
  `last_accepted_step`, `otpauth://` enrollment URI.
- **Recovery codes**: one-time codes stored only as SHA-256 hashes; the caller
  persists the hash records and gets atomic single-use semantics through
  `verify_and_consume`.
- **Assurance claims**: `amr`/`acr` helpers for step-up authorization.

No secrets are logged: `TotpSecret`'s `Debug` is redacted.
