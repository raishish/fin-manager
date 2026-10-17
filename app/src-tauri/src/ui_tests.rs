//! T3.3 end to end through the real IPC path: the renter household's TD.2 bank files
//! imported through `import_file`, then the dashboard, the review queue, a correction
//! that re-applies to past transactions, and category rename, merge and hide.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde_json::{Value, json};
use tauri::Manager;
use tauri::ipc::{CallbackFn, InvokeBody};
use tauri::test::{INVOKE_KEY, get_ipc_response};
use tauri::webview::InvokeRequest;

use crate::tests::{Webview, create_confirmed, invoke, invoke_with, mock_app};
use crate::unlock::AppUnlock;

fn exports() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/bank-exports/data")
}

/// `encodeURIComponent`.
fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn import(w: &Webview, bytes: Vec<u8>, args: &Value) -> Result<Value, Value> {
    let mut headers = tauri::http::HeaderMap::new();
    headers.insert(
        crate::importing::ARGS_HEADER,
        encode(&args.to_string()).parse().unwrap(),
    );
    let request = InvokeRequest {
        cmd: "import_file".into(),
        callback: CallbackFn(0),
        error: CallbackFn(1),
        url: "tauri://localhost".parse().unwrap(),
        body: InvokeBody::Raw(bytes),
        headers,
        invoke_key: INVOKE_KEY.to_string(),
    };
    get_ipc_response(w, request).map(|body| body.deserialize().unwrap())
}

fn ids(v: &Value) -> Vec<i64> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|i| i.as_i64().unwrap())
        .collect()
}

/// Imports the renter's 16 files, one TD.1 account at a time, creating each account from
/// its first file. Returns the number of rows per file and what the reports said.
fn import_renter(w: &Webview) -> Vec<(usize, Value)> {
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(exports().join("manifest.json")).unwrap()).unwrap();
    let mut accounts: BTreeMap<i64, i64> = BTreeMap::new();
    let mut results = Vec::new();
    for f in manifest["files"].as_array().unwrap() {
        if f["persona"] != "renter" {
            continue;
        }
        let path = f["path"].as_str().unwrap();
        let expected: Value = serde_json::from_slice(
            &std::fs::read(exports().join(f["expected"].as_str().unwrap())).unwrap(),
        )
        .unwrap();
        let td1_account = expected["source"]["account_id"].as_i64().unwrap();
        let target = match accounts.get(&td1_account) {
            Some(id) => json!({ "kind": "account", "id": id }),
            None => json!({
                "kind": "newAccount",
                "name": format!("{} {td1_account}", expected["institution"].as_str().unwrap()),
                "accountKind": expected["account"]["kind"],
                "currency": expected["account"]["currency"],
            }),
        };
        let bytes = std::fs::read(exports().join(path)).unwrap();
        let report = import(
            w,
            bytes,
            &json!({ "fileName": path, "target": target, "csvCurrency": null }),
        )
        .unwrap_or_else(|e| panic!("{path}: {e}"));
        accounts.insert(
            td1_account,
            report["statements"][0]["accountId"].as_i64().unwrap(),
        );
        results.push((expected["transactions"].as_array().unwrap().len(), report));
    }
    results
}

#[test]
fn renter_files_through_the_ui() {
    let dir = tempfile::tempdir().unwrap();
    let (app, w) = mock_app(&std::fs::canonicalize(dir.path()).unwrap());
    create_confirmed(&w);
    // The UI sends the local date; any date within a day of UTC is accepted.
    let today = crate::ledger_view::utc_date(fm_core::model::Timestamp::now()).to_string();
    let today = today.as_str();
    assert_eq!(
        invoke_with(&w, "dashboard", json!({ "today": today })).unwrap()["currency"],
        Value::Null,
        "empty ledger"
    );

    // --- Import: every row is inserted or matched, and the matches can be drilled into.
    let results = import_renter(&w);
    assert_eq!(results.len(), 16);
    let mut matched_total = 0;
    for (rows, report) in &results {
        let matched = report["matched"].as_array().unwrap();
        assert_eq!(
            report["inserted"].as_u64().unwrap() as usize + matched.len(),
            *rows,
            "every row is inserted or matched"
        );
        let matched_ids: Vec<i64> = matched
            .iter()
            .map(|m| m["transactionId"].as_i64().unwrap())
            .collect();
        let shown = invoke_with(&w, "transactions", json!({ "ids": matched_ids })).unwrap();
        assert_eq!(shown.as_array().unwrap().len(), matched_ids.len());
        matched_total += matched.len();
    }
    assert!(
        matched_total > 0,
        "the overlapping exports match existing transactions"
    );
    let paired: u64 = results
        .iter()
        .map(|(_, r)| r["transfersPaired"].as_u64().unwrap())
        .sum();
    assert!(paired > 0, "card payments and savings transfers are paired");
    assert!(
        results.iter().all(|(_, r)| r["overlapWarning"].is_null()),
        "nothing was deleted, so no overlap warning"
    );
    // The same file again is refused, with nothing written.
    let again = import(
        &w,
        std::fs::read(exports().join("chase/checking_2025.csv")).unwrap(),
        &json!({ "fileName": "again.csv", "target": { "kind": "auto" }, "csvCurrency": null }),
    )
    .unwrap_err();
    assert_eq!(again["code"], "AlreadyImported");

    // --- Dashboard: the same numbers as fm-core's views, called directly.
    let month = "2026-03";
    let dash = invoke_with(
        &w,
        "dashboard",
        json!({ "today": today, "month": month, "currency": null }),
    )
    .unwrap();
    assert_eq!(dash["currency"], "USD");
    assert_eq!(
        invoke_with(
            &w,
            "dashboard",
            json!({ "today": "1999-01-01", "month": month, "currency": null })
        )
        .unwrap_err()["code"],
        "InvalidDate",
        "\"today\" must be within a day of the clock"
    );
    assert_eq!(dash["month"], month);
    assert!(!dash["partial"].as_bool().unwrap());
    let direct = app
        .state::<AppUnlock>()
        .with_ledger(|open| {
            let ledger = open.ledger().load_ledger().unwrap();
            let query = fm_core::views::Query {
                period: fm_core::views::Period::month(month.parse().unwrap()),
                filter: fm_core::views::Filter {
                    currency: Some(fm_core::model::Currency::USD),
                    ..Default::default()
                },
                group_by: fm_core::views::GroupBy::Group,
                measure: fm_core::views::Measure::Spending,
                top: None,
                as_of: today.parse().unwrap(),
            };
            fm_core::views::breakdown(&ledger, &query).unwrap()
        })
        .unwrap();
    let totals = &dash["totals"];
    assert_eq!(
        totals["spending"]["amount"]["minor"].as_i64().unwrap(),
        direct.totals.spending.amount.minor()
    );
    assert_eq!(
        totals["netFlow"]["minor"].as_i64().unwrap(),
        direct.totals.net_flow.minor()
    );
    assert_eq!(
        totals["spending"]["amount"]["decimal"],
        direct.totals.spending.amount.to_decimal_string()
    );
    let group_totals: Vec<i64> = dash["groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["row"]["total"]["minor"].as_i64().unwrap())
        .collect();
    let direct_groups: Vec<i64> = direct.rows.iter().map(|r| r.total.minor()).collect();
    assert_eq!(group_totals, direct_groups);
    for g in dash["groups"].as_array().unwrap() {
        let in_categories: i64 = g["categories"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["row"]["total"]["minor"].as_i64().unwrap())
            .sum();
        assert_eq!(
            in_categories,
            g["row"]["total"]["minor"].as_i64().unwrap(),
            "a group's categories add up to the group (both from fm-core)"
        );
    }
    let spending_ids: BTreeSet<i64> = ids(&totals["spending"]["transactionIds"])
        .into_iter()
        .collect();
    let pending_ids = ids(&totals["pending"]["transactionIds"]);
    assert!(
        pending_ids.iter().all(|p| !spending_ids.contains(p)),
        "pending is never in a total"
    );
    assert!(!dash["trend"]["months"].as_array().unwrap().is_empty());
    assert!(
        !dash["recurring"].as_array().unwrap().is_empty(),
        "the renter's rent and subscriptions"
    );
    let months: Vec<&str> = dash["months"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m.as_str().unwrap())
        .collect();
    assert!(months.contains(&month));
    assert_eq!(
        invoke_with(&w, "dashboard", json!({ "today": today })).unwrap()["month"],
        months[0],
        "the latest month by default"
    );

    // --- Review queue: Needs review is its own line, and the queue holds it. Showing it
    // marks nothing seen: only the user's actions do (decision 46).
    let queue = invoke_with(&w, "review_queue", json!({ "offset": 0, "limit": 200 })).unwrap();
    let total = queue["total"].as_u64().unwrap();
    assert!(total > 0);
    let page = queue["transactions"].as_array().unwrap();
    assert!(
        page.iter()
            .all(|t| t["bucket"]["kind"] == "needsReview" || t["awaitsConfirmation"] == true),
        "the queue: Needs review, and decisions waiting for Approve or Reject"
    );
    assert!(
        page.iter()
            .filter(|t| t["awaitsConfirmation"] == true)
            .all(|t| t["bucket"]["kind"] == "transfer" && t["pairedWith"]["account"].is_string()),
        "without a model, what waits is the detected transfer pairs, each naming its other side"
    );
    let needs_review_line = ids(&totals["needsReview"]["transactionIds"]);
    let all_queue: BTreeSet<i64> = {
        let mut all = BTreeSet::new();
        let mut offset = 0;
        while offset < total {
            let p = invoke_with(
                &w,
                "review_queue",
                json!({ "offset": offset, "limit": 200 }),
            )
            .unwrap();
            all.extend(
                p["transactions"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|t| t["id"].as_i64().unwrap()),
            );
            offset += 200;
        }
        all
    };
    assert_eq!(all_queue.len() as u64, total);
    assert!(
        needs_review_line.iter().all(|id| all_queue.contains(id)),
        "every Needs-review transaction on the dashboard is in the queue"
    );
    let seen = app
        .state::<AppUnlock>()
        .with_ledger(|open| {
            open.ledger()
                .load_ledger()
                .unwrap()
                .decisions
                .iter()
                .filter(|d| d.seen_at.is_some())
                .count()
        })
        .unwrap();
    assert_eq!(seen, 0, "viewing lists and drill-downs marks nothing seen");

    // --- A correction re-applies to the merchant's past transactions.
    let picker = invoke(&w, "category_picker").unwrap();
    let categories: Vec<(i64, String)> = picker
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|g| g["categories"].as_array().unwrap().clone())
        .map(|c| {
            (
                c["id"].as_i64().unwrap(),
                c["name"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    let (chosen, _) = categories[0].clone();
    let (target, target_name) = categories[1].clone();
    let (fix, preview) = page
        .iter()
        .filter(|t| !t["pending"].as_bool().unwrap() && t["bucket"]["kind"] == "needsReview")
        .find_map(|t| {
            let id = t["id"].as_i64().unwrap();
            let p = invoke_with(
                &w,
                "preview_rule",
                json!({ "transactionId": id, "pattern": null }),
            )
            .unwrap();
            (p["matches"].as_u64().unwrap() >= 3).then_some((id, p))
        })
        .expect("a merchant with several transactions in the queue");
    let pattern = preview["pattern"].as_str().unwrap().to_string();
    let corrected = invoke_with(
        &w,
        "correct_transaction",
        json!({
            "transactionId": fix,
            "bucket": { "kind": "category", "categoryId": chosen },
            "merchant": null,
            "learn": true,
            "pattern": null,
        }),
    )
    .unwrap();
    assert!(corrected["ruleId"].is_i64(), "a rule was learned");
    assert!(
        corrected["reapplied"].as_u64().unwrap() >= preview["matches"].as_u64().unwrap() - 1,
        "the rule re-applied to the merchant's other transactions"
    );
    // Every transaction the rule matches now counts in the chosen category, except
    // transfer pairs, which rules never break.
    let matching: Vec<i64> = app
        .state::<AppUnlock>()
        .with_ledger(|open| {
            let ledger = open.ledger().load_ledger().unwrap();
            let matcher = fm_core::model::RuleMatcher {
                description: fm_core::model::TextMatch::Contains(pattern.clone()),
                account_id: None,
                amount: None,
            };
            ledger
                .transactions
                .iter()
                .filter(|t| fm_core::categorize::rules::matcher_matches(&matcher, t))
                .map(|t| t.id.0)
                .collect()
        })
        .unwrap();
    assert_eq!(matching.len() as u64, preview["matches"].as_u64().unwrap());
    let shown = invoke_with(&w, "transactions", json!({ "ids": matching })).unwrap();
    let mut in_chosen = 0;
    for t in shown.as_array().unwrap() {
        let kind = t["bucket"]["kind"].as_str().unwrap();
        if kind == "transfer" {
            continue;
        }
        // The corrected transaction is what the user chose; the rule fits the others to
        // their sign: money coming back from the merchant is a refund of the category.
        let money_in = t["amount"]["minor"].as_i64().unwrap() > 0;
        let expected = if t["id"] == fix || !money_in {
            "category"
        } else {
            "refund"
        };
        assert_eq!(t["bucket"]["kind"], expected, "{}", t["id"]);
        assert_eq!(t["bucket"]["categoryId"], chosen);
        in_chosen += 1;
    }
    assert!(in_chosen >= 3);
    let rules = invoke(&w, "rules").unwrap();
    let rule = &rules["rules"][0];
    assert_eq!(rule["pattern"], pattern);
    assert_eq!(rule["origin"], "correction");
    assert!(rule["decisions"].as_u64().unwrap() >= 2);

    // --- Rename: past transactions show the new name at once.
    invoke_with(
        &w,
        "edit_taxonomy",
        json!({ "edit": { "kind": "rename_category", "id": chosen, "name": "Renamed in T3.3" } }),
    )
    .unwrap();
    let shown = invoke_with(&w, "transactions", json!({ "ids": [fix] })).unwrap();
    assert_eq!(shown[0]["bucket"]["label"], "Renamed in T3.3");

    // --- Merge: past transactions move into the target, and no money is gained or lost.
    let fix_month = &shown[0]["date"].as_str().unwrap()[..7];
    let before = invoke_with(
        &w,
        "dashboard",
        json!({ "today": today, "month": fix_month, "currency": null }),
    )
    .unwrap();
    invoke_with(
        &w,
        "edit_taxonomy",
        json!({ "edit": { "kind": "merge", "from": chosen, "into": target } }),
    )
    .unwrap();
    let after = invoke_with(
        &w,
        "dashboard",
        json!({ "today": today, "month": fix_month, "currency": null }),
    )
    .unwrap();
    assert_eq!(
        after["totals"]["spending"]["amount"],
        before["totals"]["spending"]["amount"]
    );
    let shown = invoke_with(&w, "transactions", json!({ "ids": [fix] })).unwrap();
    assert_eq!(shown[0]["bucket"]["categoryId"], target);
    assert_eq!(shown[0]["bucket"]["label"], target_name);
    let in_target = after["groups"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|g| g["categories"].as_array().unwrap().clone())
        .find(|c| c["categoryId"] == target)
        .expect("the target has the merged spending");
    assert!(ids(&in_target["row"]["transactionIds"]).contains(&fix));
    let taxonomy = invoke(&w, "taxonomy").unwrap();
    let merged = taxonomy["groups"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|g| g["categories"].as_array().unwrap().clone())
        .find(|c| c["id"] == chosen)
        .unwrap();
    assert_eq!(merged["mergedInto"], target);
    let picker = invoke(&w, "category_picker").unwrap().to_string();
    assert!(
        !picker.contains("Renamed in T3.3"),
        "merged away: not offered"
    );

    // --- Hide: gone from the picker, totals unchanged.
    invoke_with(
        &w,
        "edit_taxonomy",
        json!({ "edit": { "kind": "set_category_hidden", "id": target, "hidden": true } }),
    )
    .unwrap();
    let picker = invoke(&w, "category_picker").unwrap();
    assert!(
        picker
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|g| g["categories"].as_array().unwrap().clone())
            .all(|c| c["id"] != target)
    );
    let hidden = invoke_with(
        &w,
        "dashboard",
        json!({ "today": today, "month": fix_month, "currency": null }),
    )
    .unwrap();
    assert_eq!(hidden["totals"], after["totals"]);
    // A hidden category can't be chosen for a correction.
    let refused = invoke_with(
        &w,
        "correct_transaction",
        json!({
            "transactionId": fix,
            "bucket": { "kind": "category", "categoryId": target },
            "merchant": null, "learn": false, "pattern": null,
        }),
    )
    .unwrap_err();
    assert_eq!(refused["code"], "UnknownCategory");
    // Money going out is never Income or a Refund.
    let outflow = page
        .iter()
        .find(|t| t["amount"]["minor"].as_i64().unwrap() < 0)
        .unwrap()["id"]
        .clone();
    for bucket in [
        json!({ "kind": "income" }),
        json!({ "kind": "refund", "categoryId": chosen }),
    ] {
        let refused = invoke_with(
            &w,
            "correct_transaction",
            json!({
                "transactionId": outflow,
                "bucket": bucket,
                "merchant": null, "learn": false, "pattern": null,
            }),
        )
        .unwrap_err();
        assert_eq!(refused["code"], "DoesNotFit");
    }

    // --- Rules (T2.6): disabling with "undo" re-plans what the rule decided; the
    // corrected transaction keeps the user's own decision. Deleting removes the rule.
    let rule_id = rule["id"].as_i64().unwrap();
    let rules_after = invoke_with(
        &w,
        "set_rule_enabled",
        json!({ "ruleId": rule_id, "enabled": false, "undo": true }),
    )
    .unwrap();
    let disabled = rules_after["rules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == rule_id)
        .unwrap();
    assert_eq!(disabled["enabled"], false);
    assert_eq!(disabled["decisions"], 0, "its decisions were undone");
    let shown = invoke_with(&w, "transactions", json!({ "ids": matching })).unwrap();
    for t in shown.as_array().unwrap() {
        if t["id"] == fix {
            assert_eq!(t["bucketSource"], "You", "the user's correction stays");
        } else if t["bucket"]["kind"] != "transfer" {
            assert_eq!(t["bucket"]["kind"], "needsReview", "{}", t["id"]);
        }
    }
    let rules_after = invoke_with(
        &w,
        "delete_rule",
        json!({ "ruleId": rule_id, "undo": false }),
    )
    .unwrap();
    assert!(
        rules_after["rules"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["id"] != rule_id),
        "deleted"
    );
    assert_eq!(
        invoke_with(
            &w,
            "delete_rule",
            json!({ "ruleId": rule_id, "undo": false })
        )
        .unwrap_err()["code"],
        "NotFound"
    );

    // --- Approve and Reject (decision 46) on detected transfer pairs: one pair is one
    // answer, so acting on one side changes both.
    let queue = invoke_with(&w, "review_queue", json!({ "offset": 0, "limit": 200 })).unwrap();
    // Each waiting row: its transaction, its decision, and the other side's.
    let waiting: Vec<(i64, i64, i64, i64)> = queue["transactions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["awaitsConfirmation"] == true && t["pairedWith"].is_object())
        .map(|t| {
            (
                t["id"].as_i64().unwrap(),
                t["pairedWith"]["id"].as_i64().unwrap(),
                t["bucketDecisionId"].as_i64().unwrap(),
                t["pairedWith"]["decisionId"].as_i64().unwrap(),
            )
        })
        .collect();
    let mut pairs = Vec::new();
    for p in &waiting {
        if pairs
            .iter()
            .all(|q: &(i64, i64, i64, i64)| ![q.0, q.1].contains(&p.0))
        {
            pairs.push(*p);
        }
    }
    let [a, b, c, d, e] = [pairs[0], pairs[1], pairs[2], pairs[3], pairs[4]];
    let decide = |cmd: &str, ids: &[i64]| invoke_with(&w, cmd, json!({ "decisionIds": ids }));

    assert_eq!(
        decide("reject_decisions", &[a.2]).unwrap(),
        2,
        "both sides of the pair"
    );
    let shown = invoke_with(&w, "transactions", json!({ "ids": [a.0, a.1] })).unwrap();
    for t in shown.as_array().unwrap() {
        assert_eq!(t["bucket"]["kind"], "needsReview");
        assert_eq!(t["awaitsConfirmation"], false);
    }
    assert_eq!(
        decide("reject_decisions", &[c.2, c.3]).unwrap(),
        2,
        "both sides selected is still one pair"
    );
    assert_eq!(decide("approve_decisions", &[b.2, b.3]).unwrap(), 2);
    let shown = invoke_with(&w, "transactions", json!({ "ids": [b.0, b.1] })).unwrap();
    for t in shown.as_array().unwrap() {
        assert_eq!(t["bucket"]["kind"], "transfer");
        assert_eq!(t["awaitsConfirmation"], false, "confirmed");
    }
    assert_eq!(
        decide("approve_decisions", &[b.2, b.3]).unwrap(),
        0,
        "already confirmed: nothing to do"
    );
    // Review T3.3 #2: Approve acts only on the decisions the page showed. One that
    // changed after the page loaded (here, rejected elsewhere) is refused, and nothing
    // else in the batch is confirmed.
    decide("reject_decisions", &[d.2]).unwrap();
    let changed = decide("approve_decisions", &[d.2, d.3]).unwrap_err();
    assert_eq!(changed["code"], "Changed");
    assert!(changed["message"].as_str().unwrap().contains("reload"));
    let shown = invoke_with(&w, "transactions", json!({ "ids": [d.0, d.1] })).unwrap();
    assert!(
        shown
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["bucket"]["kind"] == "needsReview"),
        "the new Needs-review decisions weren't confirmed in their place"
    );
    // Review T3.3 round 2: one side's displayed decision went stale (replaced by a new
    // one the user hasn't seen) while the other side's is still current. Reject refuses
    // the whole request rather than rejecting the new, unseen decision with the pair.
    let replaced = app
        .state::<AppUnlock>()
        .write_ledger(|open| {
            use fm_core::model::*;
            let u = open.ledger();
            let ledger = u.load_ledger().unwrap();
            let old = ledger
                .decisions
                .iter()
                .find(|x| x.id == DecisionId(e.3))
                .unwrap()
                .clone();
            u.set_decision_state(old.id, DecisionState::Superseded)
                .unwrap();
            let id = u.next_id(fm_core::storage::Table::Decisions).unwrap();
            u.insert_decision(&Decision {
                id: DecisionId(id),
                decided_at: Timestamp::now(),
                replaces: Some(old.id),
                ..old
            })
            .unwrap();
            id
        })
        .unwrap();
    let refused = decide("reject_decisions", &[e.2, e.3]).unwrap_err();
    assert_eq!(refused["code"], "Changed");
    let shown = invoke_with(&w, "transactions", json!({ "ids": [e.0, e.1] })).unwrap();
    for t in shown.as_array().unwrap() {
        assert_eq!(t["bucket"]["kind"], "transfer", "nothing was rejected");
        assert_eq!(t["awaitsConfirmation"], true);
    }
    assert_eq!(
        shown[1]["bucketDecisionId"], replaced,
        "the new decision is still open"
    );

    // --- Suggestions (T2.6): a model suggestion for a Needs-review transaction, accepted;
    // another dismissed; a third gone stale after the user corrected the transaction.
    let review = invoke_with(&w, "review_queue", json!({ "offset": 0, "limit": 200 })).unwrap();
    let open_ids: Vec<i64> = review["transactions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| {
            t["bucket"]["kind"] == "needsReview" && t["amount"]["minor"].as_i64().unwrap() < 0
        })
        .map(|t| t["id"].as_i64().unwrap())
        .take(3)
        .collect();
    let suggest_for = |tx: i64| -> i64 {
        app.state::<AppUnlock>()
            .write_ledger(|open| {
                use fm_core::model::*;
                let u = open.ledger();
                let ledger = u.load_ledger().unwrap();
                let placeholder = ledger
                    .decisions
                    .iter()
                    .find(|d| {
                        d.transaction_id == TransactionId(tx)
                            && d.state == DecisionState::Applied
                            && matches!(d.assignment, Assignment::Bucket(_))
                    })
                    .unwrap();
                let id = u.next_id(fm_core::storage::Table::Decisions).unwrap();
                u.insert_decision(&Decision {
                    id: DecisionId(id),
                    transaction_id: TransactionId(tx),
                    assignment: Assignment::Bucket(Bucket::Category {
                        category_id: CategoryId(categories[2].0),
                    }),
                    source: DecisionSource::Model(ModelProvenance {
                        model_id: "test-model".into(),
                        gguf_sha256: Sha256Hash::of(b"test model"),
                        prompt_version: "1".into(),
                        confidence: Confidence::from_basis_points(9_000).unwrap(),
                    }),
                    taxonomy_version: ledger.taxonomy.version.clone(),
                    decided_at: Timestamp::now(),
                    state: DecisionState::Suggested,
                    seen_at: None,
                    replaces: Some(placeholder.id),
                })
                .unwrap();
                id
            })
            .unwrap()
    };
    let (s1, s2, s3) = (
        suggest_for(open_ids[0]),
        suggest_for(open_ids[1]),
        suggest_for(open_ids[2]),
    );
    let listed = invoke(&w, "suggestions").unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 3);
    assert_eq!(listed[0]["suggestions"][0]["source"], "Model");

    let applied = invoke_with(&w, "accept_suggestion", json!({ "decisionId": s1 })).unwrap();
    assert_eq!(applied, json!([s1]));
    let shown = invoke_with(&w, "transactions", json!({ "ids": [open_ids[0]] })).unwrap();
    assert_eq!(shown[0]["bucket"]["categoryId"], categories[2].0);
    assert_eq!(shown[0]["bucketSource"], "Model");
    assert_eq!(
        shown[0]["awaitsConfirmation"], false,
        "accepting is acting: seen"
    );

    invoke_with(&w, "dismiss_suggestion", json!({ "decisionId": s2 })).unwrap();
    let shown = invoke_with(&w, "transactions", json!({ "ids": [open_ids[1]] })).unwrap();
    assert_eq!(shown[0]["bucket"]["kind"], "needsReview");
    assert!(shown[0]["suggestions"].as_array().unwrap().is_empty());

    invoke_with(
        &w,
        "correct_transaction",
        json!({
            "transactionId": open_ids[2],
            "bucket": { "kind": "category", "categoryId": categories[3].0 },
            "merchant": null, "learn": false, "pattern": null,
        }),
    )
    .unwrap();
    let stale = invoke_with(&w, "accept_suggestion", json!({ "decisionId": s3 })).unwrap_err();
    assert!(
        ["StaleSuggestion", "NotASuggestion"].contains(&stale["code"].as_str().unwrap()),
        "{stale}"
    );
    assert_eq!(
        invoke_with(&w, "accept_suggestion", json!({ "decisionId": s1 })).unwrap_err()["code"],
        "NotASuggestion",
        "already accepted"
    );
    assert_eq!(
        invoke_with(&w, "dismiss_suggestion", json!({ "decisionId": 999_999 })).unwrap_err()["code"],
        "NotFound"
    );
}

#[test]
fn import_errors_are_clear() {
    let dir = tempfile::tempdir().unwrap();
    let (_app, w) = mock_app(&std::fs::canonicalize(dir.path()).unwrap());
    create_confirmed(&w);
    let csv = std::fs::read(exports().join("citi/card_2025_06_16.csv")).unwrap();
    let auto = json!({ "fileName": "card.csv", "target": { "kind": "auto" }, "csvCurrency": null });
    assert_eq!(
        import(&w, csv.clone(), &auto).unwrap_err()["code"],
        "AccountNeeded",
        "a CSV doesn't say which account it is for"
    );
    assert_eq!(import(&w, vec![], &auto).unwrap_err()["code"], "Empty");
    // Review T3.3 #4: files over the cap are refused before anything is parsed.
    assert_eq!(
        import(
            &w,
            vec![b'x'; crate::importing::MAX_IMPORT_BYTES + 1],
            &auto
        )
        .unwrap_err()["code"],
        "TooLarge"
    );
    assert_eq!(
        import(&w, b"not a bank file".to_vec(), &auto).unwrap_err()["code"],
        "Unreadable"
    );
    let bad_account = json!({ "fileName": null, "target": { "kind": "account", "id": 999 }, "csvCurrency": null });
    assert_eq!(
        import(&w, csv.clone(), &bad_account).unwrap_err()["code"],
        "NotFound"
    );
    // A body without the details header is refused before anything is read.
    let no_header = InvokeRequest {
        cmd: "import_file".into(),
        callback: CallbackFn(0),
        error: CallbackFn(1),
        url: "tauri://localhost".parse().unwrap(),
        body: InvokeBody::Raw(csv),
        headers: Default::default(),
        invoke_key: INVOKE_KEY.to_string(),
    };
    let err = get_ipc_response(&w, no_header).unwrap_err();
    assert_eq!(err["code"], "BadRequest");
}
