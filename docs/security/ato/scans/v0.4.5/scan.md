# Vulnerability scan: `ghcr.io/phornstein/opentrack:v0.4.5`

- Image: `opentrack@sha256:0db93d0161b9f7991bbdab3d67554e5dfb9397b765b3d306783e4f2c71ea599c`
- Scanned: 2026-09-30 19:55 UTC
- Scanners: Version: 0.74.0; Grype 0.119.0 (databases current at scan time)
- Full reports: `trivy.json`, `grype.json` beside this file

| Severity | Findings |
|---|---|
| Critical | 2 |
| High | 8 |
| Medium | 22 |
| Low | 26 |
| Unknown | 1 |

A finding reported by both scanners is listed once. Every Critical and High finding is on the
POA&M (`../../poam.md`) unless it is fixed in the next release.

| ID | Severity | Package | Installed | Fixed in | Where | Scanner |
|---|---|---|---|---|---|---|
| CVE-2023-45853 | Critical | zlib1g | 1:1.2.13.dfsg-1 | no fix | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | trivy |
| CVE-2026-6653 | Critical | libxml2 | 2.9.14+dfsg-1.3~deb12u6 | no fix | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2026-74860 | High | libxml2 | 2.9.14+dfsg-1.3~deb12u6 | no fix | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2026-84782 | High | libssl3 | 3.0.20-1~deb12u2 | no fix | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2026-86138 | High | libxml2 | 2.9.14+dfsg-1.3~deb12u6 | no fix | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2026-86139 | High | libxml2 | 2.9.14+dfsg-1.3~deb12u6 | no fix | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2026-86140 | High | libxml2 | 2.9.14+dfsg-1.3~deb12u6 | no fix | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2026-86142 | High | libxml2 | 2.9.14+dfsg-1.3~deb12u6 | no fix | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2026-86143 | High | libxml2 | 2.9.14+dfsg-1.3~deb12u6 | no fix | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2026-86144 | High | libxml2 | 2.9.14+dfsg-1.3~deb12u6 | no fix | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2025-10911 | Medium | libxslt1.1 | 1.1.35-1+deb12u4 | no fix | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2026-18374 | Medium | libc6 | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2026-19499 | Medium | libc6 | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2026-19542 | Medium | libc6 | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2026-27171 | Medium | zlib1g | 1:1.2.13.dfsg-1 | no fix | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2026-5435 | Medium | libc6 | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2026-5450 | Medium | libc6 | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2026-5928 | Medium | libc6 | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2026-6238 | Medium | libc6 | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2026-63072 | Medium | libssl3 | 3.0.20-1~deb12u2 | 3.0.22-1~deb12u1 | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2026-63076 | Medium | libssl3 | 3.0.20-1~deb12u2 | 3.0.22-1~deb12u1 | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2026-6368 | Medium | libc6 | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2026-6791 | Medium | libc6 | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2026-76781 | Medium | libxml2 | 2.9.14+dfsg-1.3~deb12u6 | no fix | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2026-77117 | Medium | libc6 | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2026-80489 | Medium | libc6 | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2026-85091 | Medium | zlib1g | 1:1.2.13.dfsg-1 | no fix | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2026-86137 | Medium | libxml2 | 2.9.14+dfsg-1.3~deb12u6 | no fix | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2026-8674 | Medium | libc6 | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2026-86805 | Medium | libc6 | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2026-89092 | Medium | libc6 | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2026-95818 | Medium | libc6 | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2010-4756 | Low | libc6 | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2015-9019 | Low | libxslt1.1 | 1.1.35-1+deb12u4 | no fix | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2018-20796 | Low | libc6 | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2019-1010022 | Low | libc6 | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2019-1010023 | Low | libc6 | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2019-1010024 | Low | libc6 | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2019-1010025 | Low | libc6 | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2019-9192 | Low | libc6 | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2022-27943 | Low | gcc-12-base | 12.2.0-14+deb12u1 | no fix | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2022-27943 | Low | libgcc-s1 | 12.2.0-14+deb12u1 | no fix | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2022-27943 | Low | libgomp1 | 12.2.0-14+deb12u1 | no fix | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2022-27943 | Low | libstdc++6 | 12.2.0-14+deb12u1 | no fix | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2025-11731 | Low | libxslt1.1 | 1.1.35-1+deb12u4 | no fix | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2025-27587 | Low | libssl3 | 3.0.20-1~deb12u2 | no fix | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2026-11979 | Low | libxml2 | 2.9.14+dfsg-1.3~deb12u6 | no fix | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2026-35189 | Low | libssl3 | 3.0.20-1~deb12u2 | no fix | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2026-42767 | Low | libssl3 | 3.0.20-1~deb12u2 | 3.0.22-1~deb12u1 | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2026-54872 | Low | libssl3 | 3.0.20-1~deb12u2 | no fix | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2026-54874 | Low | libssl3 | 3.0.20-1~deb12u2 | 3.0.22-1~deb12u1 | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2026-63074 | Low | libssl3 | 3.0.20-1~deb12u2 | 3.0.22-1~deb12u1 | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2026-75803 | Low | libssl3 | 3.0.20-1~deb12u2 | 3.0.22-1~deb12u1 | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2026-75805 | Low | libssl3 | 3.0.20-1~deb12u2 | no fix | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2026-75806 | Low | libssl3 | 3.0.20-1~deb12u2 | no fix | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2026-77696 | Low | libssl3 | 3.0.20-1~deb12u2 | no fix | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2026-86141 | Low | libxml2 | 2.9.14+dfsg-1.3~deb12u6 | no fix | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| CVE-2026-97399 | Low | libc6 | 2.36-9+deb12u14 | no fix | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | grype, trivy |
| DLA-4792-1 | Unknown | tzdata | 2026b-0+deb12u1 | 2026c-0+deb12u1 | ghcr.io/phornstein/opentrack:v0.4.5 (debian 12.15) | trivy |
