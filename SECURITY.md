# Security policy

How to report a vulnerability in OpenTrack, what happens next, and how fixes are announced. The
product's security documents are in [`docs/security/`](docs/security/): the deployment
checklist ([hardening.md](docs/security/hardening.md)), the [threat model](docs/security/threat-model.md),
the control mapping, FIPS, the supply chain, and the accreditation package.

## Supported versions

OpenTrack is alpha. Security fixes go into the latest release line only.

| Version | Security fixes |
|---|---|
| 0.4.x, the latest release | Yes |
| Anything older | No: upgrade to the latest 0.4.x |

A fix is released as the next 0.4.x version. It is not backported to earlier releases.

## Reporting a vulnerability

**Don't put the details in an issue, pull request or discussion.** Everyone with access to the
repository can read those.

- **On GitHub:** use **Report a vulnerability** on the repository's **Security** tab (private
  vulnerability reporting).
- **By email:** write to [parker@kineticlogic.io](mailto:parker@kineticlogic.io) with "OpenTrack
  security" in the subject.
- **From a site running OpenTrack:** report through your own organisation's channel as usual; your
  ISSO or administrator forwards product vulnerabilities to the maintainer the same way.

Please include:
- the version (`/api/v1/status` or Overview → System status) and how it is deployed (image or
  source build, roles, sign-in method);
- what an attacker can do, and what they need first (network access, an account, a role);
- how to reproduce it: requests, a source spec or configuration with secrets removed, logs;
- whether it is known to anyone else, and any date you plan to publish.

Test only against a deployment you own or are allowed to test. Don't access or change other
people's data.

## What happens next

These are targets, for a small team, in working days:

| Step | Target |
|---|---|
| Acknowledge the report | 5 days |
| Confirm or reject it, with a severity and the versions affected | 10 days |
| Fix, or publish a mitigation, for a Critical or High | 30 days |
| Fix a Medium | 90 days |
| Fix a Low | the next minor release |

1. **Triage.** The maintainer reproduces the report against the latest release and rates it
   (CVSS 3.1, and the STIG category where one applies). You are told the result, and why when it
   is not treated as a vulnerability.
2. **Track.** A confirmed vulnerability is tracked privately until the fix is out. It then gets an
   issue labelled `security`, closed by the fix, as with #57 to #63.
3. **Fix.** The fix comes with a test that failed before it, and the documents it touches
   (`stig-mapping.md` findings, the accreditation evaluations and POA&M) change in the same pull
   request ([scm-plan.md](docs/security/scm-plan.md)).
4. **Release and announce** (below). You are credited, unless you would rather not be.

If a fix will take longer than the target, you are told why and when to expect it, and the
advisory gives a mitigation in the meantime.

## How fixes are announced

Each security fix is published in three places, when the release that carries it is out:

- **The release notes** of the GitHub release, and **`CHANGELOG.md`**, under **Security fixes**:
  what was wrong, who could exploit it and with what role, the fixed version, and what to do on
  upgrade.
- **An advisory** for anything rated Medium or above: a GitHub Security Advisory on the
  repository once GitHub offers them for it (they need a public repository); until then the
  release notes carry it as a **Security advisory** section with the same content: description,
  affected versions, severity, the fixed version, how to get it (the signed image by digest), and
  any interim mitigation.
- **The accreditation package** (`docs/security/ato/`): the evaluations and POA&M are updated in
  the same release.

Watch the repository's releases to be told of new ones. OpenTrack does not check for updates
itself.

## Scope

In scope: the `opentrack` binary and image, the web UI, the plugin SDKs in `sdk/`, and the forks
in `third_party/`. A vulnerability in a dependency is in scope when OpenTrack's use of it is
exploitable; otherwise report it upstream, and tell us if OpenTrack should move off the affected
version sooner than its routine updates would.

Out of scope: deployments that ignore the hardening checklist (`OT_AUTH=off`,
`insecure_skip_verify`, plaintext links on unprotected networks), which are documented risks, and
findings already on the POA&M (`docs/security/ato/poam.md`).
