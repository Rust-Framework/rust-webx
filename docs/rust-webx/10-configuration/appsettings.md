# appsettings.json ????

## ????

??????????

```
appsettings.json                    # ????
appsettings.Development.json        # ???????Development ???
```

## ?????

```json
{
  "App": {
    "Name": "My API",
    "Urls": ["http://0.0.0.0:5000"],
    "MaxBodySize": 10485760
  },
  "Jwt": {
    "Secret": "change-me"
  },
  "Cors": {
    "Origins": ["*"],
    "Methods": ["GET", "POST", "PUT", "DELETE", "OPTIONS"],
    "Headers": ["Content-Type", "Authorization"],
    "AllowCredentials": false,
    "MaxAge": 86400
  },
  "Tls": {
    "CertPath": "",
    "KeyPath": ""
  }
}
```

## ????

| ??? | Rust ?? |
|--------|----------|
| `App` | `AppSection` |
| `Jwt` | `JwtSection` |
| `Cors` | `CorsSection` |
| `Tls` | `TlsSection` |

???? `AppOptions` ????

## ????

```rust
Host::builder()
    .configure(|app| {
        app.useOptions(|opts| {
            println!("Listening on: {:?}", opts.app.urls);
        });
    })
```

## ??

`appsettings.json` ? ASP.NET Core ????????????

????[AppMode ?????](app-modes.md)
