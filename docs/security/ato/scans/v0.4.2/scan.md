# Vulnerability scan: `ghcr.io/phornstein/opentrack:v0.4.2`

- Image: `ghcr.io/phornstein/opentrack@sha256:e85c11899c0f87b3fa005cdaf743e06097625b459f00d912bc34c19ea28d6947`
- Scanned: 2026-09-30 04:24 UTC
- Scanners: Version: 0.74.0; Grype 0.119.0 (databases current at scan time)
- Full reports: `trivy.json`, `grype.json` beside this file

| Severity | Findings |
|---|---|
| Critical | 5 |
| High | 61 |
| Medium | 99 |
| Low | 90 |
| Unknown | 3 |

A finding reported by both scanners is listed once. Every Critical and High finding is on the
POA&M (`../../poam.md`) unless it is fixed in the next release.

| ID | Severity | Package | Installed | Fixed in | Where | Scanner |
|---|---|---|---|---|---|---|
| CVE-2023-45853 | Critical | zlib1g | 1:1.2.13.dfsg-1 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | trivy |
| CVE-2026-13221 | Critical | perl-base | 5.36.0-7+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-42496 | Critical | perl-base | 5.36.0-7+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-6653 | Critical | libxml2 | 2.9.14+dfsg-1.3~deb12u6 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-8376 | Critical | perl-base | 5.36.0-7+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2025-69720 | High | libtinfo6 | 6.4-4 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2025-69720 | High | ncurses-base | 6.4-4 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2025-69720 | High | ncurses-bin | 6.4-4 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-16742 | High | libsystemd0 | 252.39-1~deb12u2 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-16742 | High | libudev1 | 252.39-1~deb12u2 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-41992 | High | gzip | 1.12-1 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-42497 | High | perl-base | 5.36.0-7+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-48962 | High | perl-base | 5.36.0-7+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-53613 | High | bsdutils | 1:2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-53613 | High | libblkid1 | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-53613 | High | libmount1 | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-53613 | High | libsmartcols1 | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-53613 | High | libuuid1 | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-53613 | High | mount | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-53613 | High | util-linux | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-53613 | High | util-linux-extra | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-54369 | High | libacl1 | 2.3.1-3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-57432 | High | perl-base | 5.36.0-7+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-57433 | High | perl-base | 5.36.0-7+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-74860 | High | libxml2 | 2.9.14+dfsg-1.3~deb12u6 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-76642 | High | bsdutils | 1:2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-76642 | High | libblkid1 | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-76642 | High | libmount1 | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-76642 | High | libsmartcols1 | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-76642 | High | libuuid1 | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-76642 | High | mount | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-76642 | High | util-linux | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-76642 | High | util-linux-extra | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-78408 | High | bsdutils | 1:2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-78408 | High | libblkid1 | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-78408 | High | libmount1 | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-78408 | High | libsmartcols1 | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-78408 | High | libuuid1 | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-78408 | High | mount | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-78408 | High | util-linux | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-78408 | High | util-linux-extra | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-78409 | High | bsdutils | 1:2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-78409 | High | libblkid1 | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-78409 | High | libmount1 | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-78409 | High | libsmartcols1 | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-78409 | High | libuuid1 | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-78409 | High | mount | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-78409 | High | util-linux | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-78409 | High | util-linux-extra | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-78410 | High | bsdutils | 1:2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-78410 | High | libblkid1 | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-78410 | High | libmount1 | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-78410 | High | libsmartcols1 | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-78410 | High | libuuid1 | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-78410 | High | mount | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-78410 | High | util-linux | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-78410 | High | util-linux-extra | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-84782 | High | libssl3 | 3.0.22-1~deb12u1 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | trivy |
| CVE-2026-84782 | High | openssl | 3.0.22-1~deb12u1 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | trivy |
| CVE-2026-86138 | High | libxml2 | 2.9.14+dfsg-1.3~deb12u6 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-86139 | High | libxml2 | 2.9.14+dfsg-1.3~deb12u6 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-86140 | High | libxml2 | 2.9.14+dfsg-1.3~deb12u6 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-86142 | High | libxml2 | 2.9.14+dfsg-1.3~deb12u6 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-86143 | High | libxml2 | 2.9.14+dfsg-1.3~deb12u6 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-86144 | High | libxml2 | 2.9.14+dfsg-1.3~deb12u6 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-9538 | High | perl-base | 5.36.0-7+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2023-50495 | Medium | libtinfo6 | 6.4-4 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2023-50495 | Medium | ncurses-base | 6.4-4 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2023-50495 | Medium | ncurses-bin | 6.4-4 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2024-10041 | Medium | libpam-modules | 1.5.2-6+deb12u2 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2024-10041 | Medium | libpam-modules-bin | 1.5.2-6+deb12u2 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2024-10041 | Medium | libpam-runtime | 1.5.2-6+deb12u2 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2024-10041 | Medium | libpam0g | 1.5.2-6+deb12u2 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2025-10911 | Medium | libxslt1.1 | 1.1.35-1+deb12u4 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2025-15649 | Medium | perl-base | 5.36.0-7+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2025-30258 | Medium | gpgv | 2.2.40-1.1+deb12u2 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2025-68972 | Medium | gpgv | 2.2.40-1.1+deb12u2 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-12087 | Medium | perl-base | 5.36.0-7+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-13595 | Medium | bsdutils | 1:2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-13595 | Medium | libblkid1 | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-13595 | Medium | libmount1 | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-13595 | Medium | libsmartcols1 | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-13595 | Medium | libuuid1 | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-13595 | Medium | mount | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-13595 | Medium | util-linux | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-13595 | Medium | util-linux-extra | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-13757 | Medium | libp11-kit0 | 0.24.1-2 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-15059 | Medium | libsystemd0 | 252.39-1~deb12u2 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-15059 | Medium | libudev1 | 252.39-1~deb12u2 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-15534 | Medium | perl-base | 5.36.0-7+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-18374 | Medium | libc-bin | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-18374 | Medium | libc6 | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-18477 | Medium | tar | 1.34+dfsg-1.2+deb12u1 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-18508 | Medium | tar | 1.34+dfsg-1.2+deb12u1 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-18938 | Medium | libp11-kit0 | 0.24.1-2 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-19487 | Medium | perl-base | 5.36.0-7+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-19499 | Medium | libc-bin | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-19499 | Medium | libc6 | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-19542 | Medium | libc-bin | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-19542 | Medium | libc6 | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-27171 | Medium | zlib1g | 1:1.2.13.dfsg-1 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-27456 | Medium | bsdutils | 1:2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-27456 | Medium | libblkid1 | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-27456 | Medium | libmount1 | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-27456 | Medium | libsmartcols1 | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-27456 | Medium | libuuid1 | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-27456 | Medium | mount | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-27456 | Medium | util-linux | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-27456 | Medium | util-linux-extra | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-3184 | Medium | bsdutils | 1:2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-3184 | Medium | libblkid1 | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-3184 | Medium | libmount1 | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-3184 | Medium | libsmartcols1 | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-3184 | Medium | libuuid1 | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-3184 | Medium | mount | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-3184 | Medium | util-linux | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-3184 | Medium | util-linux-extra | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-41991 | Medium | gzip | 1.12-1 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-42250 | Medium | libbz2-1.0 | 1.0.8-5+b1 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-48959 | Medium | perl-base | 5.36.0-7+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-48961 | Medium | perl-base | 5.36.0-7+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-53615 | Medium | bsdutils | 1:2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-53615 | Medium | libblkid1 | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-53615 | Medium | libmount1 | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-53615 | Medium | libsmartcols1 | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-53615 | Medium | libuuid1 | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-53615 | Medium | mount | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-53615 | Medium | util-linux | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-53615 | Medium | util-linux-extra | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-5435 | Medium | libc-bin | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-5435 | Medium | libc6 | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-54370 | Medium | libacl1 | 2.3.1-3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-54371 | Medium | libattr1 | 1:2.5.1-4 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-54411 | Medium | libpam-modules | 1.5.2-6+deb12u2 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-54411 | Medium | libpam-modules-bin | 1.5.2-6+deb12u2 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-54411 | Medium | libpam-runtime | 1.5.2-6+deb12u2 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-54411 | Medium | libpam0g | 1.5.2-6+deb12u2 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-5450 | Medium | libc-bin | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-5450 | Medium | libc6 | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-5704 | Medium | tar | 1.34+dfsg-1.2+deb12u1 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-5928 | Medium | libc-bin | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-5928 | Medium | libc6 | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-6238 | Medium | libc-bin | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-6238 | Medium | libc6 | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-6368 | Medium | libc-bin | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-6368 | Medium | libc6 | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-6791 | Medium | libc-bin | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-6791 | Medium | libc6 | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-7010 | Medium | perl-base | 5.36.0-7+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-7017 | Medium | perl-base | 5.36.0-7+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-76781 | Medium | libxml2 | 2.9.14+dfsg-1.3~deb12u6 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-77117 | Medium | libc-bin | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-77117 | Medium | libc6 | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-80489 | Medium | libc-bin | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-80489 | Medium | libc6 | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-85091 | Medium | zlib1g | 1:1.2.13.dfsg-1 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-86137 | Medium | libxml2 | 2.9.14+dfsg-1.3~deb12u6 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-8674 | Medium | libc-bin | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-8674 | Medium | libc6 | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-86805 | Medium | libc-bin | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-86805 | Medium | libc6 | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-89092 | Medium | libc-bin | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-89092 | Medium | libc6 | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-95818 | Medium | libc-bin | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-95818 | Medium | libc6 | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2005-2541 | Low | tar | 1.34+dfsg-1.2+deb12u1 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2007-5686 | Low | login | 1:4.13+dfsg1-1+deb12u2 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2007-5686 | Low | passwd | 1:4.13+dfsg1-1+deb12u2 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2010-4756 | Low | libc-bin | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2010-4756 | Low | libc6 | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2011-3374 | Low | apt | 2.6.1 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2011-3374 | Low | libapt-pkg6.0 | 2.6.1 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2011-3389 | Low | libgnutls30 | 3.7.9-2+deb12u7 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2011-4116 | Low | perl-base | 5.36.0-7+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2013-4392 | Low | libsystemd0 | 252.39-1~deb12u2 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2013-4392 | Low | libudev1 | 252.39-1~deb12u2 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2015-9019 | Low | libxslt1.1 | 1.1.35-1+deb12u4 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2016-2781 | Low | coreutils | 9.1-1 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2017-18018 | Low | coreutils | 9.1-1 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2018-20796 | Low | libc-bin | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2018-20796 | Low | libc6 | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2018-6829 | Low | libgcrypt20 | 1.10.1-3+deb12u1 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2019-1010022 | Low | libc-bin | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2019-1010022 | Low | libc6 | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2019-1010023 | Low | libc-bin | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2019-1010023 | Low | libc6 | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2019-1010024 | Low | libc-bin | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2019-1010024 | Low | libc6 | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2019-1010025 | Low | libc-bin | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2019-1010025 | Low | libc6 | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2019-9192 | Low | libc-bin | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2019-9192 | Low | libc6 | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2022-0563 | Low | bsdutils | 1:2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2022-0563 | Low | libblkid1 | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2022-0563 | Low | libmount1 | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2022-0563 | Low | libsmartcols1 | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2022-0563 | Low | libuuid1 | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2022-0563 | Low | mount | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2022-0563 | Low | util-linux | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2022-0563 | Low | util-linux-extra | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2022-27943 | Low | gcc-12-base | 12.2.0-14+deb12u1 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2022-27943 | Low | libgcc-s1 | 12.2.0-14+deb12u1 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2022-27943 | Low | libstdc++6 | 12.2.0-14+deb12u1 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2022-3219 | Low | gpgv | 2.2.40-1.1+deb12u2 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2023-31437 | Low | libsystemd0 | 252.39-1~deb12u2 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2023-31437 | Low | libudev1 | 252.39-1~deb12u2 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2023-31438 | Low | libsystemd0 | 252.39-1~deb12u2 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2023-31438 | Low | libudev1 | 252.39-1~deb12u2 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2023-31439 | Low | libsystemd0 | 252.39-1~deb12u2 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2023-31439 | Low | libudev1 | 252.39-1~deb12u2 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2023-31486 | Low | perl-base | 5.36.0-7+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2024-2236 | Low | libgcrypt20 | 1.10.1-3+deb12u1 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2024-56433 | Low | login | 1:4.13+dfsg1-1+deb12u2 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2024-56433 | Low | passwd | 1:4.13+dfsg1-1+deb12u2 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2025-11731 | Low | libxslt1.1 | 1.1.35-1+deb12u4 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2025-13151 | Low | libtasn1-6 | 4.19.0-2+deb12u1 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2025-14104 | Low | bsdutils | 1:2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2025-14104 | Low | libblkid1 | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2025-14104 | Low | libmount1 | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2025-14104 | Low | libsmartcols1 | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2025-14104 | Low | libuuid1 | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2025-14104 | Low | mount | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2025-14104 | Low | util-linux | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2025-14104 | Low | util-linux-extra | 2.38.1-5+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2025-27587 | Low | libssl3 | 3.0.22-1~deb12u1 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2025-27587 | Low | openssl | 3.0.22-1~deb12u1 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2025-5278 | Low | coreutils | 9.1-1 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2025-6141 | Low | libtinfo6 | 6.4-4 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2025-6141 | Low | ncurses-base | 6.4-4 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2025-6141 | Low | ncurses-bin | 6.4-4 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-11979 | Low | libxml2 | 2.9.14+dfsg-1.3~deb12u6 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-35189 | Low | libssl3 | 3.0.22-1~deb12u1 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | trivy |
| CVE-2026-35189 | Low | openssl | 3.0.22-1~deb12u1 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | trivy |
| CVE-2026-40228 | Low | libsystemd0 | 252.39-1~deb12u2 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-40228 | Low | libudev1 | 252.39-1~deb12u2 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-53910 | Low | diffutils | 1:3.8-4 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-54872 | Low | libssl3 | 3.0.22-1~deb12u1 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | trivy |
| CVE-2026-54872 | Low | openssl | 3.0.22-1~deb12u1 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | trivy |
| CVE-2026-56391 | Low | coreutils | 9.1-1 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-56392 | Low | coreutils | 9.1-1 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-57062 | Low | gpgv | 2.2.40-1.1+deb12u2 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-75805 | Low | libssl3 | 3.0.22-1~deb12u1 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | trivy |
| CVE-2026-75805 | Low | openssl | 3.0.22-1~deb12u1 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | trivy |
| CVE-2026-75806 | Low | libssl3 | 3.0.22-1~deb12u1 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | trivy |
| CVE-2026-75806 | Low | openssl | 3.0.22-1~deb12u1 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | trivy |
| CVE-2026-77696 | Low | libssl3 | 3.0.22-1~deb12u1 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | trivy |
| CVE-2026-77696 | Low | openssl | 3.0.22-1~deb12u1 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | trivy |
| CVE-2026-86141 | Low | libxml2 | 2.9.14+dfsg-1.3~deb12u6 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-97399 | Low | libc-bin | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| CVE-2026-97399 | Low | libc6 | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| TEMP-0290435-0B57B5 | Low | tar | 1.34+dfsg-1.2+deb12u1 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | trivy |
| TEMP-0517018-A83CE6 | Low | sysvinit-utils | 3.06-4 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | trivy |
| TEMP-0628843-DBAD28 | Low | login | 1:4.13+dfsg1-1+deb12u2 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | trivy |
| TEMP-0628843-DBAD28 | Low | passwd | 1:4.13+dfsg1-1+deb12u2 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | trivy |
| TEMP-0841856-B18BAF | Low | bash | 5.2.15-2+b13 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | trivy |
| CVE-2026-82560 | Unknown | perl-base | 5.36.0-7+deb12u3 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | grype, trivy |
| DLA-4792-1 | Unknown | tzdata | 2026b-0+deb12u1 | 2026c-0+deb12u1 | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | trivy |
| TEMP-1149217-B31E38 | Unknown | libpcre2-8-0 | 10.42-1+deb12u1 | no fix | ghcr.io/phornstein/opentrack:v0.4.2 (debian 12.15) | trivy |
