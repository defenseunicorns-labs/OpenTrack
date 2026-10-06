# samael 0.0.20 (OpenStare fork, as used by OpenTrack)

Changes on top of the fork, for 0.4.0 accreditation work:

- `openssl_fips_enabled()` (`src/lib.rs`) reports whether OpenSSL's default
  library context is in FIPS mode. OpenTrack refuses SAML sign-in unless it is
  (docs/security/fips.md).
- quick-xml goes from 0.37 to 0.42, which fixes RUSTSEC-2026-0194 and
  RUSTSEC-2026-0195 (denial of service while parsing XML). One call site
  changed: `root_element_local_name`.
