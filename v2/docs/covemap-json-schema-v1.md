# COVE-MAP JSON Schema v1

This is the reference implementation companion schema for JSON payloads inside
`.covemap` artifacts and embedded `MAP_*` sections. It targets the COVE-MAP v2
artifact framing and defines the JSON fields that `cove-map` validates,
replays, converts, explains, and projects.

## Row Semantics

`MAP_ROW_SEMANTICS_CATALOG.rules[]` supports:

- `rule_id`, `source_id`, `identity_rule_id`: required identifiers.
- `row_semantics_kind`: optional, defaults to `Object`; if present it must be
  one of `Object`, `EventObject`, `LinkObject`, `AssociationOnly`,
  `Composite`, `Dispatched`, `KeyValueFragment`, `ProjectionOnly`,
  `EvidenceOnly`, or `Tombstone`.
- `assertion_kinds`: required non-empty array declaring the finite assertion
  kinds the rule may produce. Supported values are `object`, `property`,
  `association`, `temporal`, `identity_key`, `identity_equivalence`,
  `candidate_match`, `tombstone`, `evidence`, `conflict`, and `projection`.
- `tombstone_target`: required for `Tombstone` rules and invalid on other row
  kinds. Supported values are `object`, `property`, `association`,
  `source_record`, and `evidence`.
- `record_kind`: optional COVE-O row kind, defaults to `Baseline`.
- `temporal_policy`: optional, defaults to `latest_committed`.
- `conflict_policy`: optional, defaults to `reject_conflict`.
- `property_bindings`: object property materialization rules.
- `association_bindings`: link-object materialization rules for COVE-O v2.

The reference materializer creates object rows for `Object`, `EventObject`,
`LinkObject`, `Composite`, `Dispatched`, `KeyValueFragment`, and `Tombstone`.
`AssociationOnly` may create association/link objects without a separate object
row for the source identity. `ProjectionOnly` and `EvidenceOnly` do not create
canonical object rows.

Property bindings support:

- `assertion_id`, `property_id`, `property_name`, `source_column`,
  `logical_type`: required.
- `physical_kind`: optional, defaults to `auto`.
- `value_expression`: optional, defaults to `source_column`; currently supports
  `source.<column>` and direct source column names.
- `nullable`: optional, defaults to `true`.
- `missing_policy`: optional, `null` or `reject`; defaults to `null`.
- `conflict_policy`: optional, defaults to `reject_conflict`.
- `source_priority`: optional integer override for this property binding. Lower
  values win under `source_priority_wins`.

Property conflict policies are finite:

- `reject_conflict`: unequal non-null values for the same destination
  GOID/property fail conversion. Duplicate equal values are allowed. Null values
  do not overwrite non-null values.
- `source_priority_wins`: chooses the non-null candidate with the lowest
  priority, then source declaration order, source row index, and assertion id.
  Losing values are suppressed from object properties but retained in the MAP
  evidence index.

Association bindings support:

- `assertion_id`, `association_type`, `target_identity_rule_id`: required.
- `source_identity_rule_id`: optional; defaults to the row rule identity.
- `source_role`, `target_role`: optional endpoint labels.
- `source_endpoint_expression`, `target_endpoint_expression`: optional,
  defaulting to `source.goid` and `target.goid`. Endpoint expressions are
  limited to `source.goid`, `target.goid`, and `identity(<identity_rule_id>)`.
- `valid_from_expression`, `valid_to_expression`: optional temporal endpoint
  expressions.
- `cardinality_policy`: optional, defaults to `one`.
- `missing_policy`: optional, `reject` or `skip`; defaults to `reject`.
- `link_object_materialization`: optional, defaults to `required`.

Association output is materialized as COVE-O object types named
`Association:<association_type>` with required properties `source_goid`,
`target_goid`, `association_type`, `mapping_rule_id`, `source_evidence_id`,
`source_role`, `target_role`, `valid_from`, `valid_to`, and
`cardinality_policy`. Validity values are preserved as JSON values from the
declared finite source expression grammar. If a declared validity expression is
missing and `missing_policy` is `reject`, conversion fails; with `skip`, that
association is skipped. COVE-O commit timestamps remain independent from these
association validity fields.

## Source Catalog and Governance

`MAP_SOURCE_CATALOG` supports optional
`governance_reconciliation_policy`, defaulting to
`emit_effective_policy`. Supported policies are:

- `emit_effective_policy`: preserve effective governance metadata in the MAP
  conversion report.
- `reject_on_mixed_sensitivity`: reject conversion if materialized output
  combines sources with different non-empty sensitivity labels or ranks.

Source entries support optional governance and priority fields:

- `source_priority`: integer source priority. Lower values win unless a
  property binding overrides the priority.
- `sensitivity_label`: source sensitivity label.
- `sensitivity_rank`: integer sensitivity rank. Higher ranks are more
  restrictive for effective-policy reporting.
- `access_policy_ids`: array of policy identifiers.

The conversion report emits `governance` with per-source policy metadata,
`effective_sensitivity_rank`, labels at that rank, and the union of
`access_policy_ids`.

## Identity Rules

Identity rules support optional `auto_merge`. Merge defaults are:

- `authoritative`: `auto_merge` defaults to `true`.
- `strong_deterministic`: `auto_merge` defaults to `false`.
- `source_scoped`: merges only within a single `source_id`.
- `weak_deterministic`, `candidate`, and `candidate_only`: do not form GOID
  merge edges.

Join-key hash inputs use the canonical §70.5 tuple: marker, object type id,
identity rule id, component count, and each ordered component role, logical
type, and explicit null marker or length-prefixed canonical value bytes.

Candidate rules still compute canonical join-key evidence. They are reported in
`plan-keys.candidate_matches`, `MAP_CONVERSION_REPORT.candidate_matches`, and
`MAP_EVIDENCE_INDEX` entries with `candidate: true`, but they never enter
union-find, GOID selection, identity-equivalence output, or COVE-O object row
materialization.

## Source Fingerprints

For local CSV/JSONL replay checks, the reference CLI recognizes:

- `snapshot_digest`: `sha256:<64 lowercase hex>` over the exact source file
  bytes.
- `schema_fingerprint`: `cove-map-schema-v1:<64 lowercase hex>` over source
  kind, sorted field names, and observed JSON primitive kind sets.

When `replay_claimed` is true, both recognized fingerprints are required and
must match the live source input.

## Projection Catalog

`MAP_PROJECTION_CATALOG.projections[]` supports the expanded schema below.
Legacy entries containing only `projection_id` and `assertion_ids` still parse
as metadata, but `cove-map project` rejects them because they do not define a
deterministic read surface.

Required for executable projections:

- `projection_id`
- `output_table`
- `row_grain`
- `anchor`
- `temporal_mode`
- `multi_value_policy`
- `columns`
- `output_modes`

Executable projection `output_modes` are `json`, `arrow`, `cove-t`, and
`sql`. `cove-o` is accepted only as a schema declaration: table-to-object
projection semantics are not defined by the v2 reference executor, so
`cove-map project --format cove-o` fails closed with a precise error.

Supported row grains:

- `one_row_per_object`
- `one_row_per_association`
- `one_row_per_link_object`
- `one_row_per_property_version`
- `one_row_per_event_object`
- `one_row_per_object_as_of_time`
- `one_row_per_evidence_assertion`

Projection anchors declare exactly one of:

- `anchor.object_type`
- `anchor.association_type`

Projection columns use:

- `name`: output column name.
- `value`: expression over the object, association, or evidence surface.
- `logical_type`: optional declared output type.
- `conflict_policy`: optional, defaults to `canonical_value`.
- `missing_policy`: optional, defaults to `null`.

Missing `logical_type` defaults to `utf8`. Executable projections support
scalar output types only: bool, signed and unsigned integer widths, float32/64,
date days, timestamp micros/nanos, decimal64/128, utf8, binary, json, and uuid.
Nested list/struct/map output declarations remain schema-only and are rejected
by executable projection output paths.

Supported `temporal_mode` values are `latest_committed`, `full_history`,
`valid_time`, `observed_time`, and `commit_order`. Supported
`multi_value_policy` declarations are `reject`, `explode`, `aggregate`,
`first`, `last`, and `list`; the current executable projector implements
`reject`, association-row `explode`, and `count(association(...))` aggregation,
and fails closed for unsupported declared behavior.

The reference projection expression grammar is intentionally finite:

- `object.goid`, `Object.goid`, or `goid`
- `object_type`, `object.type`, or `Object.type`
- `<ObjectType>.<property_name>`
- `association.goid`
- `association.source_goid`
- `association.target_goid`
- `association.association_type`
- `association.mapping_rule_id`
- `association.source_evidence_id`
- `association.source_role`
- `association.target_role`
- `association.valid_from`
- `association.valid_to`
- `association.cardinality_policy`
- `evidence.<field>`
- `count(association(<association_type>))`

Unsupported expressions fail closed with `MAP_INVALID`.
