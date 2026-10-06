# rumqttc 0.25.1 (patched)

This is published rumqttc 0.25.1 with one change, in `Cargo.toml`: it uses
`rustls-webpki` 0.103.13 or later, without default features, in place of
0.102.

- **FIPS:** the webpki `ring` backend is no longer built.
- **Advisories:** RUSTSEC-2026-0049, -0098, -0099 and -0104 in webpki 0.102
  no longer apply.

rumqttc uses only webpki's error type. TLS goes through the rustls
configuration OpenTrack passes in, which uses the AWS-LC FIPS provider.

Remove this copy once an upstream release drops webpki 0.102.
