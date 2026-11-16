"""Publish tested native images and resolve signed-release candidates by digest."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
from urllib.parse import quote

REPOSITORY = "defenseunicorns-labs/opentrack"
ARCHITECTURES = ("amd64", "arm64")
DOCKER_TAG = re.compile(r"[A-Za-z0-9_][A-Za-z0-9_.-]{0,127}")


def reserved_tag(value):
    return (
        value == "latest"
        or value.startswith(("ci-", "sha-"))
        or re.match(r"^v[0-9]+\.[0-9]+\.[0-9]+(?:$|[.-])", value, re.IGNORECASE) is not None
    )


def branch_name(ref):
    if not isinstance(ref, str) or not ref.startswith("refs/heads/"):
        raise ValueError("Development image publication requires a branch ref")
    branch = ref.removeprefix("refs/heads/")
    if (
        not branch or branch.startswith("/") or branch.endswith(("/", "."))
        or any(value in branch for value in ("..", "//", "@{"))
        or re.search(r"[\x00-\x20\x7f~^:?*\[\\]", branch)
        or any(part.startswith(".") or part.endswith(".lock") for part in branch.split("/"))
    ):
        raise ValueError("Invalid branch ref")
    return branch


def branch_tag(branch):
    branch_name("refs/heads/" + branch)
    # Preserve normal branch names. Reserve the generated suffix form so a
    # literal branch cannot take another branch's normalized/hash-suffixed tag.
    if (DOCKER_TAG.fullmatch(branch) and not reserved_tag(branch)
            and not re.search(r"--[0-9a-f]{16}$", branch)):
        return branch
    slug = re.sub(r"[^A-Za-z0-9_.-]+", "-", branch).lstrip(".-") or "branch"
    if reserved_tag(slug):
        slug = "branch-" + slug
    suffix = hashlib.sha256(branch.encode()).hexdigest()[:16]
    return f"{slug[:110]}--{suffix}"


def image_name(repository):
    if repository.lower() != REPOSITORY:
        raise ValueError("Container publication is restricted to the OpenTrack repository")
    return f"ghcr.io/{REPOSITORY}"


def commit(value):
    if not re.fullmatch(r"[0-9a-f]{40}", value):
        raise ValueError("Expected a full lowercase Git commit SHA")
    return value


def digest(value):
    if not re.fullmatch(r"sha256:[0-9a-f]{64}", value):
        raise ValueError("Expected a sha256 image digest")
    return value


def publication_channel(repository, ref):
    image_name(repository)
    return branch_tag(branch_name(ref))


def release_tag(value):
    # Docker tags cannot contain SemVer '+' build metadata.
    if not re.fullmatch(r"v(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)(?:-[A-Za-z0-9][A-Za-z0-9.-]*)?", value):
        raise ValueError("Expected a version tag vMAJOR.MINOR.PATCH[-prerelease]")
    if len(value) > 128:
        raise ValueError("Release tag exceeds the container tag length limit")
    return value


def record(repository, revision, architecture, image_digest):
    if architecture not in ARCHITECTURES:
        raise ValueError("Unsupported architecture")
    return {
        "image": image_name(repository),
        "revision": commit(revision),
        "architecture": architecture,
        "digest": digest(image_digest),
    }


def native_references(directory, repository, revision):
    paths = sorted(Path(directory).glob("*.json"))
    if {path.name for path in paths} != {f"{arch}.json" for arch in ARCHITECTURES}:
        raise ValueError("Expected exactly one tested digest record per architecture")
    references = []
    for path in paths:
        item = json.loads(path.read_text())
        if item != record(repository, revision, path.stem, item["digest"]):
            raise ValueError("Native image metadata does not match this workflow revision")
        references.append(f"{item['image']}@{item['digest']}")
    if len(set(references)) != len(ARCHITECTURES):
        raise ValueError("Native image digests must be distinct")
    return references


def verify_manifest(manifest):
    digest(manifest["digest"])
    platforms = []
    for entry in manifest["manifests"]:
        digest(entry["digest"])
        platform = entry.get("platform", {})
        if entry.get("annotations", {}).get("vnd.docker.reference.type") == "attestation-manifest":
            if platform.get("os") != "unknown" or platform.get("architecture") != "unknown":
                raise ValueError("Unexpected attestation platform")
            continue
        platforms.append((platform.get("os"), platform.get("architecture")))
    if sorted(platforms) != [("linux", arch) for arch in ARCHITECTURES]:
        raise ValueError("Image must contain exactly linux/amd64 and linux/arm64")
    return manifest["digest"]


def inspect(reference):
    return json.loads(subprocess.check_output(
        ["docker", "buildx", "imagetools", "inspect", reference, "--format", "{{json .Manifest}}"],
        text=True,
    ))


def publish(directory, repository, revision, channel, promote_channel=True, branch=None):
    branch = branch if branch is not None else channel
    if channel != branch_tag(branch):
        raise ValueError("Development channel must match the safe tag for its branch")
    image = image_name(repository)
    references = native_references(directory, repository, revision)
    # Different branches/reruns of one commit must not race on the candidate
    # index tag before it has been inspected. These are not deployment tags.
    candidate_key = hashlib.sha256("\0".join([branch, *references]).encode()).hexdigest()
    candidate = f"{image}:ci-{commit(revision)}-{candidate_key}-index"
    command = ["docker", "buildx", "imagetools", "create"]
    subprocess.run(command + [
        "--tag", candidate,
        "--annotation", f"index:org.opencontainers.image.source=https://github.com/{repository}",
        "--annotation", f"index:org.opencontainers.image.revision={revision}",
        "--annotation", f"index:io.opentrack.image.branch={branch}",
        *references,
    ], check=True)
    image_digest = verify_manifest(inspect(candidate))
    reference = f"{image}@{image_digest}"
    tag = f"{image}:sha-{revision}"
    tags = ["--tag", tag]
    if promote_channel:
        tags += ["--tag", f"{image}:{channel}"]
    subprocess.run(command + [*tags, reference], check=True)
    return {
        "image": image, "revision": revision, "reference": reference,
        "tag": tag, "channel": channel,
        "branch": branch,
        "branch_reference": f"{image}:{channel}",
        "platforms": [f"linux/{arch}" for arch in ARCHITECTURES],
        "native_references": references,
        "release_signed": False,
        "channel_updated": promote_channel,
    }


def successful_build_record(repository, revision):
    """Read metadata from this repo's successful protected-branch image workflow."""
    image_name(repository)
    revision = commit(revision)
    runs = json.loads(subprocess.check_output([
        "gh", "run", "list", "--repo", repository, "--workflow", "images.yml",
        "--commit", revision, "--event", "push", "--status", "success",
        "--limit", "100", "--json", "databaseId,headBranch,headSha",
    ], text=True))
    approved = [run for run in runs if run["headSha"] == revision and run["headBranch"] in ("main", "release")]
    if not approved:
        raise ValueError("A successful main/release container-images run is required for this source commit")
    with tempfile.TemporaryDirectory(prefix="opentrack-ci-record-") as directory:
        subprocess.run([
            "gh", "run", "download", str(approved[0]["databaseId"]), "--repo", repository,
            "--name", "opentrack-image-references", "--dir", directory,
        ], check=True)
        return json.loads((Path(directory) / "image-references.json").read_text())


def resolve_release(repository, revision, tag, build_record):
    """Resolve an already-built image; never build, sign, or promote a version tag."""
    image = image_name(repository)
    tag = release_tag(tag)
    revision = commit(revision)
    if (
        build_record.get("image") != image
        or build_record.get("revision") != revision
        or build_record.get("channel") not in ("main", "release")
        or build_record.get("branch", build_record.get("channel")) != build_record.get("channel")
        or build_record.get("platforms") != [f"linux/{arch}" for arch in ARCHITECTURES]
    ):
        raise ValueError("Successful CI build record does not match the release source")
    reference = build_record["reference"]
    if not reference.startswith(image + "@"):
        raise ValueError("Unexpected release image repository")
    recorded_digest = digest(reference.split("@", 1)[1])
    manifest = inspect(reference)
    image_digest = verify_manifest(manifest)
    if image_digest != recorded_digest:
        raise ValueError("Resolved image digest differs from the successful CI build record")
    annotations = manifest.get("annotations", {})
    if annotations.get("org.opencontainers.image.revision") != revision:
        raise ValueError("CI image revision does not match the release source commit")
    if annotations.get("org.opencontainers.image.source", "").lower() != f"https://github.com/{REPOSITORY}":
        raise ValueError("CI image is not associated with the OpenTrack repository")
    if annotations.get("io.opentrack.image.branch", build_record["channel"]) != build_record["channel"]:
        raise ValueError("CI image branch does not match the approved release build record")
    return {
        "image": image, "revision": revision, "release_tag": tag,
        "reference": f"{image}@{image_digest}",
        "platforms": [f"linux/{arch}" for arch in ARCHITECTURES],
    }


def main():
    parser = argparse.ArgumentParser()
    commands = parser.add_subparsers(dest="command", required=True)
    metadata = commands.add_parser("metadata")
    metadata.add_argument("--output", type=Path, required=True)
    native = commands.add_parser("record")
    native.add_argument("--architecture", choices=ARCHITECTURES, required=True)
    native.add_argument("--digest", required=True)
    native.add_argument("--output", type=Path, required=True)
    combined = commands.add_parser("publish")
    combined.add_argument("--digests", type=Path, required=True)
    combined.add_argument("--output", type=Path, required=True)
    release = commands.add_parser("release-candidate")
    release.add_argument("--revision", required=True)
    release.add_argument("--tag", required=True)
    release.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    repository = os.environ["GITHUB_REPOSITORY"]
    if args.command == "metadata":
        image = image_name(repository)
        channel = publication_channel(repository, os.environ.get("GITHUB_REF"))
        with args.output.open("a") as stream:
            stream.write(f"image={image}\nbranch-tag={channel}\n")
        print(f"Branch image: {image}:{channel}")
        return
    if args.command == "record":
        result = record(repository, os.environ["GITHUB_SHA"], args.architecture, args.digest)
    elif args.command == "publish":
        if os.environ.get("GITHUB_EVENT_NAME") != "push":
            raise ValueError("Development image publication requires a trusted push event")
        ref = os.environ.get("GITHUB_REF")
        channel = publication_channel(repository, ref)
        branch_head = subprocess.check_output(
            ["gh", "api", f"repos/{repository}/git/ref/{quote(ref.removeprefix('refs/'), safe='/')}", "--jq", ".object.sha"],
            text=True,
        ).strip()
        result = publish(args.digests, repository, os.environ["GITHUB_SHA"], channel,
                         promote_channel=commit(branch_head) == os.environ["GITHUB_SHA"],
                         branch=branch_name(ref))
        if summary := os.environ.get("GITHUB_STEP_SUMMARY"):
            with Path(summary).open("a") as stream:
                stream.write(
                    f"## OpenTrack development image\n\n`{result['reference']}`\n\n"
                    f"Platforms: {', '.join(result['platforms'])}\n\n"
                    f"Branch image: `{result['branch_reference']}`\n\n"
                    "**Not a signed versioned release.**\n"
                )
        print(result["reference"])
    else:
        release_tag(args.tag)
        result = resolve_release(
            repository, args.revision, args.tag,
            successful_build_record(repository, args.revision),
        )
    args.output.write_text(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    main()
