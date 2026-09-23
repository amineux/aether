# Red-team diligence clip

Host stdout a buyer can grep. Reuses `run_blast_demo`, `run_blast_hops_demo`,
`run_blast_nodes_demo`, `run_bank_color_demo`, `run_uncolored_compute_demo`,
`run_foreign_tenant_color_demo`, `run_qos_credits_demo`, `run_fence_not_ready_demo`, `run_outside_slice_demo`, `run_typed_window_sid_demo`,
`run_silent_remote_demo`, `run_hbm_bw_demo`, `run_xqueue_sid_override_demo`, `run_set_sid_unbound_demo`, `run_submit_sid_demo`, `run_sid_budget_demo`, `run_stage2_fault_demo`, `run_softnoi_exhausted_demo`,
`run_hodge_harmonic_tree_demo`, `run_hodge_curl_tree_demo`, `run_hodge_quota_demo`, `run_firewall_demo`, `run_firewall_ident_pa_demo`, `run_greenctx_overcommit_demo`, `run_greenctx_unbound_demo`, `run_greenctx_exhausted_demo`, `run_greenctx_busy_demo`, `run_smmu_overlap_demo`, `run_softsfi_demo`,
`run_softnoi_demo`, and `run_sva_demo`.
Not a new isolator, not a QEMU guest, not a slide. Blast-nodes needle:
`[redteam] attack=blast-nodes result=refused` (`admit_nodes` → `BlastRadius`;
hops stays `attack=blast-hops`). Bank-color needle:
`[redteam] attack=bank-color result=refused` (`admit_wave` →
`ColorError::ForeignBank`; Exchange still OK). Uncolored-compute needle:
`[redteam] attack=uncolored-compute result=refused` (`admit_wave(..., color=None)` →
`ColorError::Uncolored`; Exchange with color still OK; not ForeignBank /
bank-color). Foreign-tenant-color needle:
`[redteam] attack=foreign-tenant-color result=refused` (`admit_wave` →
`ColorError::ForeignTenant`; Exchange still OK; not ForeignBank / bank-color
or Uncolored / uncolored-compute). QoS credits needle:
`[redteam] attack=qos-credits result=refused` (`Timeline::submit` → `CreditExhausted`).
Fence-not-ready needle: `[redteam] attack=fence-not-ready result=refused`
(`Timeline::wait` → `FenceNotReady`; not CreditExhausted / qos-credits;
timeout-frees-credit stays inside the qos demo).
Outside-slice needle: `[redteam] attack=outside-slice result=refused`
(`admit_chiplet` → `OutsideSlice`; not hops / qos / CrossCut / bank-color).
Silent-remote needle: `[redteam] attack=silent-remote result=refused`
(`map_place` / `map_fabric` → `SilentRemoteLoad`; `MEM_FULL` never implies
`UNIFIED`; not CXL productization / BAR0 / SoftNPU).
Soft HBM BW needle: `[redteam] attack=hbm-bw result=refused`
(`SoftHbmBwMeter::charge` → `QosExceeded` vs `QosBudget.bw_mbps` on HBM
`TypedWindow`; software meter only).
XQueue SID override needle: `[redteam] attack=xqueue-sid-override result=refused`
(`stamp_queue_sid` second SID → `HalError::Busy` on pending Soft-CP XQueue;
not BAR0 / SoftNPU).
SET_SID unbound needle: `[redteam] attack=set-sid-unbound result=refused`
(Soft-CP `set_sid` / submit without Bound SID → `HalError::Fault`; SID-at-submit
`StreamAbort` foundation; not xqueue-sid-override / PASID).
Submit-sid needle: `[redteam] attack=submit-sid result=refused`
(Soft-SMMU `resolve_submit` without SET_SID → `MapError::SubmitSid`; walk still OK;
not set-sid-unbound / SidBudget / PASID).
Sid-budget needle: `[redteam] attack=sid-budget result=refused`
(Soft-SMMU `bind_stream` over `SID_BUDGET_PER_TENANT` → `MapError::SidBudget`;
peer tenant still has budget; not set-sid-unbound / SubmitSid / PASID).
Stage2-fault needle: `[redteam] attack=stage2-fault result=refused`
(Soft-SMMU `unbind_stage2` then nested walk → `MapError::Stage2Fault`; SID stays Bound;
not PASID stale / SubmitSid / StreamAbort).
SoftNoI-exhausted needle: `[redteam] attack=softnoi-exhausted result=refused`
(`admit` past `MAX_NOI_TENANTS` → `NoiError::Exhausted`; not softnoi-is OverBudget /
fabric-class RingExhausted).
Hodge harmonic-tree needle: `[redteam] attack=hodge-harmonic-tree result=refused`
(`OperatorKernelHandle::bind(Tree, Harmonic)` → `HodgeError::HarmonicTreeReduce`;
not SoftNoI fabric-class Curl ring).
Hodge curl-tree needle: `[redteam] attack=hodge-curl-tree result=refused`
(`OperatorKernelHandle::bind(Tree, Curl)` → `HodgeError::CurlOnTree`; sibling of
HarmonicTreeReduce; not SoftNoI fabric-class Curl ring).
Hodge-quota needle: `[redteam] attack=hodge-quota result=refused`
(`HodgeQuota::empty().admit(...)` → `HodgeError::QuotaExceeded`; generous admit succeeds;
not HarmonicTreeReduce / CurlOnTree / ClassNotAuthorized / CapTable).
Firewall-ident-pa needle: `[redteam] attack=firewall-ident-pa result=refused`
(SoftCmdFirewall `admit_packed` identity guest PA → `HalError::Fault`;
not mutation-during-validate — `softcmdfirewall` stays separate; not
confidential GPU).
Greenctx-overcommit needle: `[redteam] attack=greenctx-overcommit result=refused`
(`SoftGreenPool::create` past SM/WQ pool → `GreenCtxError::Overcommit`; not diligence
`run_greenctx_demo` 70/30 sell; not HW MIG / BAR0 / SoftNPU).
Greenctx-unbound needle: `[redteam] attack=greenctx-unbound result=refused`
(`SoftGreenPool::migrate_to_yield` on unbound queue → `GreenCtxError::Unbound`; not
set-sid-unbound Soft-CP Fault; not greenctx-overcommit; not HW MIG / BAR0 / SoftNPU).
Greenctx-exhausted needle: `[redteam] attack=greenctx-exhausted result=refused`
(`SoftGreenPool::create` past `MAX_GREEN_CTX` slots → `GreenCtxError::Exhausted`; not
greenctx-overcommit SM/WQ ceiling; not SoftNoI Exhausted; not HW MIG / BAR0 / SoftNPU).
Greenctx-busy needle: `[redteam] attack=greenctx-busy result=refused`
(`SoftGreenPool::migrate_to_yield` when dest bound to another queue → `GreenCtxError::Busy`; not greenctx-unbound; not overcommit/exhausted; not xqueue-sid-override Soft-CP Busy; not HW MIG / BAR0 / SoftNPU).
SoftSFI tensor needle: `[softsfi] tensor=refused` (`SoftOp::Tensor` → `Unmodeled`;
Smmu-overlap needle: `[redteam] attack=smmu-overlap result=refused`
(Soft-SMMU `map` same-SID overlapping guest PA → `MapError::Overlap`; disjoint admits; not CrossTenant / WrongStream / Stage2Fault / SubmitSid / SidBudget).
heap line stays separate).
SoftSFI unknown needle: `[softsfi] unknown=refused` / `[softsfi] unknown-base=refused` (bad opcode / illegal width →
`Unmodeled`; tensor/heap lines stay separate; not AddImm deepen).

Typed-window-sid needle: `[redteam] attack=typed-window-sid result=refused`
(`map_window_sid` → `WrongStream`; exploration TypedWindow stub, not CXL.mem
silicon / BAR0; CrossTenant on foreign pin).

From the repo root:

```bash
make red-team
```

or `cargo run -p aether-redteam`. CI greps `[redteam] attack=… result=refused`
plus `[redteam] fabric-class admit/refuse`, `[redteam] ATOMIC_ADD accept/reject`,
`[softsfi] tensor=refused`, `[softsfi] heap=refused`, `[softsfi] unknown=refused`, and the “what this is not” closer. See
[docs/DILIGENCE.md](../../docs/DILIGENCE.md).
