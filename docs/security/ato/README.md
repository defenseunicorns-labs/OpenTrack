# Accreditation package (DoD RMF)

What a site needs, beside the product's own security documents, to put OpenTrack through an
Authorization to Operate: a **generic** package for NIST SP 800-53 Rev 5 **Moderate**, the
Application Security and Development (ASD) STIG and the Container Platform SRG. It names no
authorising official or hosting environment: `[[SITE: ...]]` marks what the adopting site fills
in, and every checklist item the site must answer is left *Not Reviewed* with who answers it.

| File | What it is |
|---|---|
| [`ssp-narratives.md`](ssp-narratives.md) | A narrative for each of the 287 Moderate controls: how OpenTrack meets it, what the site does, status and responsibility (OpenTrack, Shared or Site). For the SSP. |
| [`checklists/asd-v6r4.ckl`](checklists/) | ASD STIG V6R4 (1 Oct 2025), every rule evaluated. STIG Viewer 2 format; STIG Viewer 3 and eMASS import it. |
| [`checklists/container-platform-srg-v2r4.ckl`](checklists/) | Container Platform SRG V2R4 (28 Oct 2025). Written for the platform: its runtime, orchestrator and registry items are the site's; the image's are evaluated. |
| [`checklists/summary.md`](checklists/summary.md) | Counts per status, and every open item. |
| [`scans/<version>/`](scans/) | Vulnerability scans of each release image (Trivy and Grype): full JSON reports and a summary. |
| [`poam.md`](poam.md), [`poam.csv`](poam.csv) | Plan of Action and Milestones for every open finding: checklist items, partially implemented controls, scan findings. The CSV has the DoD POA&M template's columns, for eMASS. |
| `evaluations/*.json`, `ssp/controls.json`, `poam.json` | The sources the checklists and narratives are generated from. Change these, never the generated files. |

The product documents they build on:
- [`../stig-mapping.md`](../stig-mapping.md): control mapping and findings F-1 to F-9;
- [`../hardening.md`](../hardening.md): the deployment checklist;
- [`../fips.md`](../fips.md) and [`../supply-chain.md`](../supply-chain.md);
- [`../threat-model.md`](../threat-model.md): STRIDE per interface, and the criticality analysis;
- [`../scm-plan.md`](../scm-plan.md): software configuration management, releases, roles;
- [`../coding-standards.md`](../coding-standards.md): the rules the code is written to;
- [`SECURITY.md`](../../../SECURITY.md): vulnerability reporting, response and advisories;
- the [admin guide](../../guides/admin.md), including its
  [ports, protocols and services](../../guides/admin.md#ports-protocols-and-services) table.

## What the adopting site does

1. **Fill in the placeholders** in `ssp-narratives.md` (`[[SITE: ...]]`): system name and
   boundary, the AO, hosting, the organisation-defined parameters (review frequencies, retention
   in the SIEM, and so on).
2. **Answer the Not Reviewed items** in the checklists: each one's comment says who (host OS,
   platform, ISSO process, collector and SIEM, ...). Open the `.ckl` in STIG Viewer, set the asset
   (host name, IP, marking), and record the site's answer.
3. **Deploy by the hardening checklist** ([`../hardening.md`](../hardening.md)): TLS everywhere,
   sign-in on (`OT_AUTH=on`), no `insecure_skip_verify`, OpenTelemetry export to the site's
   collector, encrypted `/data`, and the rest. Several checklist answers assume it.
4. **Scan the image you deploy** (below) and carry its findings into the POA&M.
5. **Adopt the POA&M**: its milestones are the product's; add the site's own.

## Regenerate for a release

```sh
scripts/ato/scan ghcr.io/kineticlogic-io/opentrack:v<version>   # scans/<version>/
scripts/ato/checklist                                      # checklists/ from evaluations/
scripts/ato/ssp                                            # ssp-narratives.md from ssp/controls.json
scripts/ato/poam                                           # poam.md, poam.csv from poam.json
```

- **Scan** every release image; list its Critical and High findings in the POA&M, or fix them.
- **Re-evaluate** the rules a release touches: edit `evaluations/*.json` (status, finding
  details, comments) in the same pull request as the change, as with the docs.
- **A new STIG release:** add it to `BENCHMARKS` in `scripts/ato/checklist`; the script names
  every rule that is new or gone, and refuses to write a checklist until each is evaluated.
- Update `stig-mapping.md` and the POA&M when a finding is fixed or opened.

## How the evaluations were made

Each rule was evaluated against the code and documents at the version above, citing the file or
guide section that shows it:
- **Not a finding:** OpenTrack meets it as shipped.
- **Open:** OpenTrack as shipped does not meet a requirement that applies to it. On the POA&M.
- **Not applicable:** it can't apply (a technology OpenTrack doesn't use), with the reason.
- **Not reviewed:** the site, platform or organisation answers it, or it needs a check on the
  deployed system; the comment says who.

The evaluations are the product team's, for an assessor to verify, not an assessment.
