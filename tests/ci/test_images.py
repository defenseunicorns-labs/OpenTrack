import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parents[2] / "scripts/ci/images.py"
spec = importlib.util.spec_from_file_location("ci_images", SCRIPT)
images = importlib.util.module_from_spec(spec)
spec.loader.exec_module(images)

REPO = "defenseunicorns-labs/OpenTrack"
REVISION = "a" * 40
AMD64, ARM64, INDEX = [f"sha256:{char * 64}" for char in "123"]


def manifest():
    return {
        "digest": INDEX,
        "annotations": {
            "org.opencontainers.image.revision": REVISION,
            "org.opencontainers.image.source": f"https://github.com/{REPO}",
        },
        "manifests": [
            {"digest": AMD64, "platform": {"os": "linux", "architecture": "amd64"}},
            {"digest": ARM64, "platform": {"os": "linux", "architecture": "arm64", "variant": "v8"}},
            {"digest": "sha256:" + "4" * 64,
             "platform": {"os": "unknown", "architecture": "unknown"},
             "annotations": {"vnd.docker.reference.type": "attestation-manifest"}},
        ],
    }


def build_record():
    return {
        "image": images.image_name(REPO), "revision": REVISION, "channel": "main",
        "platforms": ["linux/amd64", "linux/arm64"],
        "reference": f"{images.image_name(REPO)}@{INDEX}",
    }


class ImageTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        for arch, value in (("amd64", AMD64), ("arm64", ARM64)):
            self.write_record(arch, value)

    def write_record(self, arch, value, revision=REVISION):
        (self.root / f"{arch}.json").write_text(json.dumps(
            images.record(REPO, revision, arch, value),
        ))

    def test_image_belongs_to_upstream_repository(self):
        self.assertEqual(images.image_name(REPO), "ghcr.io/defenseunicorns-labs/opentrack")
        for repository in ("kineticlogic-io/OpenTrack", "another/OpenTrack", "defenseunicorns/uds-cuas"):
            with self.subTest(repository=repository), self.assertRaises(ValueError):
                images.image_name(repository)

    def test_repository_branches_can_publish(self):
        for ref, channel in (("main", "main"), ("release", "release"),
                             ("feature-123", "feature-123"), ("Fix_UI.v2", "Fix_UI.v2")):
            self.assertEqual(images.publication_channel(REPO, f"refs/heads/{ref}"), channel)
        for ref in (None, "", "refs/pull/3/merge", "refs/tags/v1.0.0",
                    "refs/heads/", "refs/heads/a..b", "refs/heads/a\nb",
                    "refs/heads/a/.hidden", "refs/heads/a.lock", "refs/heads/a//b"):
            with self.subTest(ref=ref), self.assertRaises(ValueError):
                images.publication_channel(REPO, ref)

    def test_normalized_branch_tags_are_valid_stable_and_distinct(self):
        branches = ("feat/ci-multiarch-ghcr", "feat-ci-multiarch-ghcr", "修正/地図",
                    "x" * 200, "latest", "ci-test", "sha-test", "v1.0.0", "v1.0.0-rc.1")
        tags = [images.branch_tag(branch) for branch in branches]
        self.assertEqual(len(tags), len(set(tags)))
        for branch, tag in zip(branches, tags):
            with self.subTest(branch=branch):
                self.assertRegex(tag, r"^[A-Za-z0-9_][A-Za-z0-9_.-]{0,127}$")
                self.assertEqual(tag, images.branch_tag(branch))
                self.assertFalse(images.reserved_tag(tag))
        normalized = images.branch_tag(branches[0])
        self.assertNotEqual(images.branch_tag(normalized), normalized)
        self.assertRegex(normalized, r"^feat-ci-multiarch-ghcr--[0-9a-f]{16}$")

    def test_full_commit_and_digest_are_required(self):
        with self.assertRaises(ValueError):
            images.commit("abc123")
        for value in ("latest", "sha256:123", "sha256:" + "x" * 64):
            with self.subTest(value=value), self.assertRaises(ValueError):
                images.digest(value)

    def test_digest_records_are_from_the_same_source_commit(self):
        self.assertEqual(
            images.native_references(self.root, REPO, REVISION),
            [f"{images.image_name(REPO)}@{value}" for value in (AMD64, ARM64)],
        )
        self.write_record("arm64", ARM64, "b" * 40)
        with self.assertRaises(ValueError):
            images.native_references(self.root, REPO, REVISION)

    def test_missing_extra_and_duplicate_records_are_rejected(self):
        self.write_record("arm64", AMD64)
        with self.assertRaises(ValueError):
            images.native_references(self.root, REPO, REVISION)
        (self.root / "arm64.json").unlink()
        with self.assertRaises(ValueError):
            images.native_references(self.root, REPO, REVISION)
        self.write_record("arm64", ARM64)
        (self.root / "extra.json").write_text("{}")
        with self.assertRaises(ValueError):
            images.native_references(self.root, REPO, REVISION)

    def test_both_platforms_and_attestations_are_accepted(self):
        self.assertEqual(images.verify_manifest(manifest()), INDEX)

    def test_wrong_missing_and_duplicate_runtime_platforms_are_rejected(self):
        for arch in ("amd64", "s390x", "unknown"):
            value = manifest()
            value["manifests"][1]["platform"]["architecture"] = arch
            with self.subTest(arch=arch), self.assertRaises(ValueError):
                images.verify_manifest(value)

    @patch.object(images, "inspect")
    @patch.object(images.subprocess, "run")
    def test_development_publication_cannot_promote_a_version(self, run, inspect):
        inspect.return_value = manifest()
        result = images.publish(self.root, REPO, REVISION, "main")
        self.assertFalse(result["release_signed"])
        self.assertEqual(run.call_count, 2)
        tags = run.call_args.args[0]
        self.assertIn(f"{images.image_name(REPO)}:main", tags)
        self.assertNotIn(f"{images.image_name(REPO)}:release", tags)
        self.assertEqual(tags[-1], f"{images.image_name(REPO)}@{INDEX}")
        with self.assertRaises(ValueError):
            images.publish(self.root, REPO, REVISION, "v1.0.0")

    @patch.object(images, "inspect")
    @patch.object(images.subprocess, "run")
    def test_branch_publication_uses_only_its_own_tag(self, run, inspect):
        inspect.return_value = manifest()
        branch = "feat/ci-multiarch-ghcr"
        channel = images.branch_tag(branch)
        result = images.publish(self.root, REPO, REVISION, channel, branch=branch)
        tags = run.call_args.args[0]
        self.assertIn(f"{images.image_name(REPO)}:{channel}", tags)
        self.assertNotIn(f"{images.image_name(REPO)}:main", tags)
        self.assertEqual(result["branch"], branch)
        self.assertEqual(result["branch_reference"], f"{images.image_name(REPO)}:{channel}")
        first_candidate = run.call_args_list[0].args[0][5]
        run.reset_mock()
        images.publish(self.root, REPO, REVISION, "other")
        self.assertNotEqual(first_candidate, run.call_args_list[0].args[0][5])

    @patch.object(images.subprocess, "run")
    def test_mismatched_branch_channel_cannot_publish(self, run):
        with self.assertRaises(ValueError):
            images.publish(self.root, REPO, REVISION, "main", branch="feature")
        run.assert_not_called()

    @patch.object(images, "inspect")
    @patch.object(images.subprocess, "run")
    def test_incomplete_index_never_promotes_a_channel(self, run, inspect):
        value = manifest()
        value["manifests"] = value["manifests"][:1]
        inspect.return_value = value
        with self.assertRaises(ValueError):
            images.publish(self.root, REPO, REVISION, "main")
        self.assertEqual(run.call_count, 1)

    def test_release_tag_cannot_be_a_branch_or_shell_option(self):
        self.assertEqual(images.release_tag("v1.0.0-rc.1"), "v1.0.0-rc.1")
        for value in ("main", "release", "--help", "v01.0.0", "v1.0.0+metadata"):
            with self.subTest(value=value), self.assertRaises(ValueError):
                images.release_tag(value)

    @patch.object(images, "inspect")
    @patch.object(images.subprocess, "run")
    def test_release_resolution_does_not_build_sign_or_publish(self, run, inspect):
        inspect.return_value = manifest()
        candidate = images.resolve_release(REPO, REVISION, "v1.0.0", build_record())
        self.assertEqual(candidate["reference"], f"{images.image_name(REPO)}@{INDEX}")
        self.assertEqual(candidate["release_tag"], "v1.0.0")
        run.assert_not_called()

    @patch.object(images, "inspect")
    def test_release_candidate_requires_matching_revision_and_repository(self, inspect):
        for field, value in (("org.opencontainers.image.revision", "b" * 40),
                             ("org.opencontainers.image.source", "https://github.com/another/repo")):
            data = manifest()
            data["annotations"][field] = value
            inspect.return_value = data
            with self.subTest(field=field), self.assertRaises(ValueError):
                images.resolve_release(REPO, REVISION, "v1.0.0", build_record())

    @patch.object(images, "inspect")
    def test_release_rejects_branch_test_and_tampered_build_records(self, inspect):
        for field, value in (("channel", "branch-ci-multiarch-ghcr"),
                             ("branch", "feature"),
                             ("revision", "b" * 40),
                             ("reference", "ghcr.io/another/image@" + INDEX),
                             ("platforms", ["linux/amd64"])):
            data = build_record()
            data[field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                images.resolve_release(REPO, REVISION, "v1.0.0", data)
        inspect.assert_not_called()

    @patch.object(images, "inspect")
    def test_release_rejects_feature_branch_index(self, inspect):
        data = manifest()
        data["annotations"]["io.opentrack.image.branch"] = "feature"
        inspect.return_value = data
        with self.assertRaises(ValueError):
            images.resolve_release(REPO, REVISION, "v1.0.0", build_record())

    @patch.object(images, "inspect")
    def test_release_checks_the_exact_digest_from_ci_not_a_tag(self, inspect):
        inspect.return_value = manifest()
        images.resolve_release(REPO, REVISION, "v1.0.0", build_record())
        inspect.assert_called_once_with(build_record()["reference"])
        data = manifest()
        data["digest"] = "sha256:" + "5" * 64
        inspect.return_value = data
        with self.assertRaises(ValueError):
            images.resolve_release(REPO, REVISION, "v1.0.0", build_record())

    @patch.object(images, "inspect")
    @patch.object(images.subprocess, "run")
    def test_historical_rebuild_does_not_roll_back_a_branch_channel(self, run, inspect):
        inspect.return_value = manifest()
        result = images.publish(self.root, REPO, REVISION, "main", promote_channel=False)
        self.assertFalse(result["channel_updated"])
        self.assertNotIn(f"{images.image_name(REPO)}:main", run.call_args.args[0])
        self.assertIn(f"{images.image_name(REPO)}:sha-{REVISION}", run.call_args.args[0])

    @patch.object(images.subprocess, "check_output")
    @patch.object(images.subprocess, "run")
    def test_only_successful_protected_branch_runs_can_supply_release_records(self, run, output):
        output.return_value = json.dumps([
            {"headBranch": "feat/ci-multiarch-ghcr", "headSha": REVISION, "databaseId": 1},
        ])
        with self.assertRaises(ValueError):
            images.successful_build_record(REPO, REVISION)
        run.assert_not_called()

        output.return_value = json.dumps([
            {"headBranch": "main", "headSha": REVISION, "databaseId": 2},
        ])
        def download(command, **kwargs):
            target = Path(command[command.index("--dir") + 1])
            (target / "image-references.json").write_text(json.dumps(build_record()))
        run.side_effect = download
        self.assertEqual(images.successful_build_record(REPO, REVISION), build_record())
        self.assertIn("2", run.call_args.args[0])


class ReleaseWorkflowPolicyTests(unittest.TestCase):
    def test_signing_is_required_before_version_promotion(self):
        workflow = SCRIPT.parents[2].joinpath(".github/workflows/release.yml").read_text()
        gate = workflow.index("- name: Require the upstream repository and release signing configuration")
        resolve = workflow.index("- name: Resolve and verify the already-built multi-platform CI image")
        sign = workflow.index("- name: Sign (OpenSSL FIPS provider)")
        promote = workflow.index("- name: Promote the version tag only after signature verification")
        self.assertLess(gate, resolve)
        self.assertLess(resolve, sign)
        self.assertLess(sign, promote)
        self.assertIn("unsigned releases are not permitted", workflow)
        self.assertIn('scripts/release/sign-image "$ref"', workflow)
        self.assertNotIn("continue-on-error:", workflow)

    def test_release_reuses_ci_output_instead_of_recompiling_the_application(self):
        workflow = SCRIPT.parents[2].joinpath(".github/workflows/release.yml").read_text()
        self.assertIn("scripts/ci/images.py release-candidate", workflow)
        self.assertNotIn("docker/build-push-action@", workflow)
        self.assertIn("ghcr.io/defenseunicorns-labs/opentrack", workflow)
        self.assertNotIn("ghcr.io/kineticlogic-io/opentrack", workflow)
