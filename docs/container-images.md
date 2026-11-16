# Container delivery

OpenTrack owns its image build:

```text
OpenTrack source → native GitHub Actions builds → ghcr.io/defenseunicorns-labs/opentrack
                                                        ↓
                                              deployment repositories
```

Deployment repositories such as `uds-cuas` should consume an approved digest;
they should not rebuild OpenTrack or require its source on a deployment host.
This change does not update those consumers automatically.

## Native CI builds

`.github/workflows/images.yml` builds:

| Platform | Runner |
| --- | --- |
| `linux/amd64` | `ubuntu-24.04` |
| `linux/arm64` | `ubuntu-24.04-arm` |

The same source commit is built on each native runner, with BuildKit provenance
and SBOM attestations. Each runner executes the resulting binary with `--help`
inside a read-only, capability-dropped container without container network
access. This is a startup smoke test, not a database/SSO/full-stack test.
The existing source CI remains separate and must also pass before a release.

Only two successful native smoke tests permit publication of a combined index.
The helper combines tested **digests**, validates exactly the two runtime
platforms (while retaining attestation descriptors), and then publishes:

```text
ghcr.io/defenseunicorns-labs/opentrack:sha-<full-source-commit>
ghcr.io/defenseunicorns-labs/opentrack:main
ghcr.io/defenseunicorns-labs/opentrack:release
ghcr.io/defenseunicorns-labs/opentrack:<branch-tag>
```

Branch tags reflect their respective branch; a historical workflow rerun
can refresh a commit image but cannot roll a newer branch channel backwards.
Deploy using the combined index digest from the workflow summary or the
`opentrack-image-references` artifact:

```text
ghcr.io/defenseunicorns-labs/opentrack@sha256:<index-digest>
```

The `ci-<commit>-amd64`, `ci-<commit>-arm64`, and `ci-<commit>-<hash>-index` tags are
intermediates and may remain after a partial failure. They are not deployment
references.

**These CI builds are development images, not signed versioned releases.**
BuildKit attestations do not replace the project's release-signing policy.
Do not treat `main` or a `sha-...` tag as an approved signed-release baseline.

## Triggers

Every branch push in the upstream repository builds both architectures, even
for documentation-only changes. Forks, branch deletions, and Dependabot pushes
do not publish. PR events run lightweight metadata tests in source CI; a PR
branch pushed within this repository also gets its own image through the push
workflow. Existing branches get a tag on their next push after adopting this
workflow. Deleting a branch does not automatically delete its registry tag.

Docker-valid branch names such as `main`, `release`, and `fix-login` are used
unchanged. Names containing `/`, Unicode, or more than 128 characters are
sanitized and get `--` plus the first 16 hex characters of their SHA-256 hash.
For example, `feat/my-feature` maps to `feat-my-feature--<16-hex-hash>`.
The slug is truncated to keep the result within Docker's 128-character limit.
The exact tag is printed in the workflow summary and recorded as
`branch_reference` in the image-reference artifact.

`latest`, `ci-*`, `sha-*`, version-like names (`v1.2.3`, including prereleases),
and names ending in the generated `--<16-hex-hash>` form are reserved.
Those branches also receive a safe hashed tag rather than overwriting release,
internal, or another normalized branch tag. Reserved prefixes get a `branch-`
prefix. Branch builds never publish version tags or sign releases; only
successful `main` / `release` runs may supply signed-release candidates.

## Signed releases

After merging a version's source, wait for its protected-branch `container-images`
run and the normal source CI to pass before publishing its GitHub release.
`.github/workflows/release.yml` then:

1. Requires the existing `RELEASE_SIGNING_KEY` and `RELEASE_SIGNING_PASSWORD`.
   A missing signing setup fails closed, without publishing an unsigned version
   tag.
2. Reads the image reference artifact from a **successful `images.yml` push run**
   for the exact release commit on `main` / `release`. It does not trust a
   mutable registry tag as the source of the release digest.
3. Inspects that recorded digest, checks its source/repository metadata and both
   architectures, then signs and verifies the **combined index** using the
   existing `scripts/release/sign-image` / OpenSSL FIPS provider process.
4. Only after signature verification, promotes `vMAJOR.MINOR.PATCH[-prerelease]`
   to that same digest and attaches `image-digest.txt` and the source SBOMs to the
   GitHub release. No application recompilation occurs in this workflow.

Signing still uses the release source's `cosign.pub` and signing scripts. No new
key is generated, no transparency-log policy is changed, and no signing secret
is required by ordinary branch builds. Maintainers must configure the existing
project key in this repository; this pipeline does not bypass that requirement.

CI image-reference artifacts are retained for 90 days. If a record has expired,
rerun the successful native image workflow for that source commit before
retrying a release. Reruns overwrite only their own metadata artifacts and do
not roll a newer branch channel back. Historical releases without a successful
protected-branch native build cannot be automatically promoted by this workflow.
Keep signed release assets for long-term traceability, not just Actions artifacts.

Verify versioned releases as described in
[Supply chain](security/supply-chain.md#releases). Deployment systems should
verify the release signature and pin its index digest before rolling it out.

## Registry permissions and visibility

Publishing uses the repository's short-lived `GITHUB_TOKEN` with `packages: write`,
not a personal access token or a deployment-machine password. The image's source
metadata links it to this repository. Organization policy must permit package
creation and the native runner labels; an existing package may need explicit
Actions access for this repository.

A public GitHub repository does **not** automatically make its GHCR package
public. Review the package's actual visibility after first publication; only a
maintainer should change it. Consumers may need GHCR read authentication.
Do not put credentials in image references, build arguments, Git, or deployment
inventory.

## Local checks

```sh
python3 -B -m unittest discover -s tests/ci -p 'test_*.py'
actionlint .github/workflows/images.yml .github/workflows/release.yml .github/workflows/ci.yml
```

These commands do not publish an image, create a release, or provision a host.
