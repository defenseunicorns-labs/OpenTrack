// Vendored (OpenStare's fork of samael 0.0.20): its own warnings are not ours to fix here.
#![allow(warnings)]
pub mod attribute;
pub mod crypto;
pub mod idp;
pub mod key_info;
pub mod metadata;
pub mod schema;
pub mod service_provider;
pub mod signature;

pub mod traits;

#[macro_use]
extern crate derive_builder;

/// OpenTrack (FIPS): whether OpenSSL's default library context fetches only
/// FIPS-approved algorithms, i.e. `default_properties = fips=yes` with the
/// FIPS provider loaded. xmlsec checks SAML signatures through OpenSSL, so
/// sign-in is refused unless this holds. OpenSSL 3 only.
pub fn openssl_fips_enabled() -> bool {
    openssl_sys::init();
    // Safety: a read-only query of the default (null) library context.
    unsafe { openssl_sys::EVP_default_properties_is_fips_enabled(std::ptr::null_mut()) == 1 }
}
