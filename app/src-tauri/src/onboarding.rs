//! First run (T5.5, SPEC decision 34): where onboarding stands, the first insight from
//! fixed logic, and the backup reminder.
//!
//! - **Step:** stored in the ledger's `meta` (`onboarding.step`), so quitting part-way
//!   resumes there. A ledger without the key has just been created: onboarding starts at
//!   bringing in data. The Welcome and password steps come before a ledger exists and
//!   are T5.1's create flow.
//! - **First insight:** [`fm_core::views::first_look`] over the last 12 months of the
//!   imported data, plus recurring charges and the detected transfer pairs. No model and
//!   no categories needed, so it is there minutes after the first import.
//! - **Backup reminder:** see [`reminder`].

use std::collections::BTreeSet;

use fm_core::model::{Assignment, Bucket, Date, DecisionState, Ledger, Timestamp, YearMonth};
use fm_core::unlock::OpenLedger;
use fm_core::views::{self, FirstLookQuery, Key, Period};
use serde::Serialize;

use crate::backup::BackupStatus;
use crate::dashboard::{LineView, RecurringView, main_currency, per_currency, recurring_charges};
use crate::ledger_view::{MoneyView, Result, counting_date, local_today, locked, ui_error};
use crate::unlock::AppUnlock;

const STEP_KEY: &str = "onboarding.step";
const SNOOZE_KEY: &str = "backup.reminder_snoozed_until";

/// The steps after the ledger exists, in order (SPEC §first run 3–7).
pub const STEPS: &[&str] = &["data", "insight", "review", "backup", "ask", "done"];

/// Months the first insight covers, ending with the latest month of data.
pub const INSIGHT_MONTHS: i32 = 12;
/// Merchants listed before "Other".
pub const INSIGHT_MERCHANTS: usize = 10;
/// How long "Remind me tomorrow" hides the backup reminder.
pub const SNOOZE_SECONDS: i64 = 24 * 60 * 60;
/// A backup older than this is worth a reminder (backups normally run daily).
pub const STALE_SECONDS: i64 = 7 * 24 * 60 * 60;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OnboardingView {
    /// One of [`STEPS`].
    pub step: &'static str,
    pub done: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowMonthView {
    pub month: String,
    pub partial: bool,
    pub money_in: LineView,
    pub money_out: LineView,
    pub transfers_in: LineView,
    pub transfers_out: LineView,
    pub pending: LineView,
    pub net_flow: MoneyView,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MerchantView {
    pub label: String,
    /// Named by the bank's description: no merchant name decided yet.
    pub from_description: bool,
    pub money_out: MoneyView,
    pub count: usize,
    pub transaction_ids: Vec<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InsightView {
    /// No transactions yet.
    pub empty: bool,
    pub currency: Option<&'static str>,
    pub first_month: Option<String>,
    pub last_month: Option<String>,
    pub months: Vec<FlowMonthView>,
    pub money_in: Option<LineView>,
    pub money_out: Option<LineView>,
    pub transfers_out: Option<LineView>,
    pub pending: Option<LineView>,
    pub merchants: Vec<MerchantView>,
    pub merchants_other: Option<MerchantView>,
    pub recurring: Vec<RecurringView>,
    /// Transfer pairs found between the user's own accounts.
    pub transfer_pairs: usize,
    /// How many transactions are in the ledger.
    pub transactions: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ReminderReason {
    /// No backup folder chosen.
    NoFolder,
    /// The last backup failed.
    Failed,
    /// A folder is set but no backup has been made yet.
    NoBackupYet,
    /// The last backup is older than [`STALE_SECONDS`].
    Stale,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReminderView {
    /// `None`: nothing to remind about (or snoozed).
    pub reason: Option<ReminderReason>,
    /// Unix seconds, when a snooze ends.
    pub snoozed_until: Option<i64>,
}

/// The backup reminder rule. Nothing while snoozed. Otherwise: no folder; the last
/// backup failed; a folder but no backup yet; or a last backup older than a week.
/// Backups run on their own once a folder is set (daily and after each import), so a
/// reminder clears itself once one succeeds.
pub fn reminder(
    folder: Option<&str>,
    last_at: Option<i64>,
    last_error: Option<&str>,
    snoozed_until: Option<i64>,
    now: i64,
) -> Option<ReminderReason> {
    if snoozed_until.is_some_and(|until| until > now) {
        return None;
    }
    if folder.is_none() {
        return Some(ReminderReason::NoFolder);
    }
    if last_error.is_some() {
        return Some(ReminderReason::Failed);
    }
    match last_at {
        None => Some(ReminderReason::NoBackupYet),
        Some(at) if now - at > STALE_SECONDS => Some(ReminderReason::Stale),
        Some(_) => None,
    }
}

fn line(l: &views::FlowMonth) -> FlowMonthView {
    FlowMonthView {
        month: l.month.to_string(),
        partial: l.partial,
        money_in: (&l.money_in).into(),
        money_out: (&l.money_out).into(),
        transfers_in: (&l.transfers_in).into(),
        transfers_out: (&l.transfers_out).into(),
        pending: (&l.pending).into(),
        net_flow: l.net_flow.into(),
    }
}

fn merchant(m: &views::MerchantOut) -> MerchantView {
    MerchantView {
        label: m.label.clone(),
        from_description: matches!(
            m.key,
            Key::Merchant {
                from_description: true,
                ..
            }
        ),
        money_out: m.money_out.into(),
        count: m.count,
        transaction_ids: m.transaction_ids.iter().map(|t| t.0).collect(),
    }
}

/// The first insight: the last [`INSIGHT_MONTHS`] months of data in the main currency.
pub fn insight(ledger: &Ledger, as_of: Date) -> Result<InsightView> {
    let per = per_currency(ledger);
    let Some(currency) = main_currency(&per) else {
        return Ok(InsightView {
            empty: true,
            currency: None,
            first_month: None,
            last_month: None,
            months: vec![],
            money_in: None,
            money_out: None,
            transfers_out: None,
            pending: None,
            merchants: vec![],
            merchants_other: None,
            recurring: vec![],
            transfer_pairs: 0,
            transactions: 0,
        });
    };
    let accounts: BTreeSet<_> = ledger
        .accounts
        .iter()
        .filter(|a| a.currency == currency)
        .map(|a| a.id)
        .collect();
    let last: YearMonth = ledger
        .transactions
        .iter()
        .filter(|t| accounts.contains(&t.account_id))
        .map(|t| counting_date(t).year_month())
        .max()
        .expect("the main currency has transactions");
    let first = last
        .checked_add_months(-(INSIGHT_MONTHS - 1))
        .ok_or_else(|| ui_error("InvalidMonth", "month out of range"))?;
    let look = views::first_look(
        ledger,
        &FirstLookQuery {
            period: Period::months(first, last)?,
            filter: views::Filter {
                currency: Some(currency),
                ..views::Filter::default()
            },
            top: INSIGHT_MERCHANTS,
            as_of,
        },
    )?;
    // Each detected pair is two applied transfer decisions naming each other.
    let paired = ledger
        .decisions
        .iter()
        .filter(|d| {
            d.state == DecisionState::Applied
                && matches!(
                    d.assignment,
                    Assignment::Bucket(Bucket::Transfer {
                        counterpart: Some(_)
                    })
                )
        })
        .count();
    Ok(InsightView {
        empty: false,
        currency: Some(currency.code()),
        first_month: Some(first.to_string()),
        last_month: Some(last.to_string()),
        months: look.months.iter().map(line).collect(),
        money_in: Some((&look.money_in).into()),
        money_out: Some((&look.money_out).into()),
        transfers_out: Some((&look.transfers_out).into()),
        pending: Some((&look.pending).into()),
        merchants: look.merchants.iter().map(merchant).collect(),
        merchants_other: look.other.as_ref().map(merchant),
        recurring: recurring_charges(ledger, currency, as_of),
        transfer_pairs: paired / 2,
        transactions: ledger.transactions.len(),
    })
}

fn step_of(stored: Option<String>) -> &'static str {
    // Unknown values (a damaged setting) start onboarding again rather than skip it.
    stored
        .as_deref()
        .and_then(|s| STEPS.iter().find(|step| **step == s))
        .copied()
        .unwrap_or(STEPS[0])
}

impl AppUnlock {
    pub fn onboarding(&self) -> Result<OnboardingView> {
        self.with_ledger(|open| {
            let step = step_of(open.setting(STEP_KEY)?);
            Ok(OnboardingView {
                step,
                done: step == "done",
            })
        })
        .ok_or_else(locked)?
    }

    pub fn set_onboarding_step(&self, step: &str) -> Result<OnboardingView> {
        let step = STEPS
            .iter()
            .find(|s| **s == step)
            .copied()
            .ok_or_else(|| ui_error("InvalidStep", "not an onboarding step"))?;
        self.with_ledger(|open| {
            open.set_setting(STEP_KEY, Some(step))?;
            Ok(OnboardingView {
                step,
                done: step == "done",
            })
        })
        .ok_or_else(locked)?
    }

    pub fn first_insight(&self, today: &str) -> Result<InsightView> {
        let as_of = local_today(today, Timestamp::now())?;
        self.with_ledger(|open| insight(&open.ledger().load_ledger()?, as_of))
            .ok_or_else(locked)?
    }

    pub fn backup_reminder(&self) -> Result<ReminderView> {
        let status: BackupStatus = self
            .backup_status()
            .map_err(|_| ui_error("Backup", "the backup status couldn't be read"))?;
        let snoozed_until = self
            .with_ledger(|open| open.setting(SNOOZE_KEY))
            .ok_or_else(locked)??
            .and_then(|s| s.parse::<i64>().ok());
        Ok(ReminderView {
            reason: reminder(
                status.folder.as_deref(),
                status.last_at,
                status.last_error.as_deref(),
                snoozed_until,
                Timestamp::now().unix_seconds(),
            ),
            snoozed_until,
        })
    }

    pub fn snooze_backup_reminder(&self) -> Result<ReminderView> {
        let until = Timestamp::now().unix_seconds() + SNOOZE_SECONDS;
        self.with_ledger(|open| open.set_setting(SNOOZE_KEY, Some(&until.to_string())))
            .ok_or_else(locked)??;
        self.backup_reminder()
    }
}

pub mod commands {
    use tauri::State;

    use super::{InsightView, OnboardingView, ReminderView};
    use crate::ledger_view::Result;
    use crate::unlock::AppUnlock;

    #[tauri::command]
    pub fn onboarding_state(unlock: State<'_, AppUnlock>) -> Result<OnboardingView> {
        unlock.onboarding()
    }

    #[tauri::command]
    pub fn set_onboarding_step(
        unlock: State<'_, AppUnlock>,
        step: String,
    ) -> Result<OnboardingView> {
        unlock.set_onboarding_step(&step)
    }

    /// `today` is the local date from the UI (`ledger_view::local_today`).
    #[tauri::command(async)]
    pub fn first_insight(unlock: State<'_, AppUnlock>, today: String) -> Result<InsightView> {
        unlock.first_insight(&today)
    }

    #[tauri::command(async)]
    pub fn backup_reminder(unlock: State<'_, AppUnlock>) -> Result<ReminderView> {
        unlock.backup_reminder()
    }

    #[tauri::command(async)]
    pub fn snooze_backup_reminder(unlock: State<'_, AppUnlock>) -> Result<ReminderView> {
        unlock.snooze_backup_reminder()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_792_000_000;

    #[test]
    fn backup_reminder_rules() {
        let r = |folder: Option<&str>,
                 last: Option<i64>,
                 err: Option<&str>,
                 snooze: Option<i64>| { reminder(folder, last, err, snooze, NOW) };
        assert_eq!(r(None, None, None, None), Some(ReminderReason::NoFolder));
        assert_eq!(
            r(Some("/b"), None, None, None),
            Some(ReminderReason::NoBackupYet)
        );
        assert_eq!(
            r(Some("/b"), Some(NOW - 60), Some("disk"), None),
            Some(ReminderReason::Failed)
        );
        assert_eq!(r(Some("/b"), Some(NOW - 60), None, None), None, "all good");
        assert_eq!(
            r(Some("/b"), Some(NOW - STALE_SECONDS - 1), None, None),
            Some(ReminderReason::Stale)
        );
        assert_eq!(r(Some("/b"), Some(NOW - STALE_SECONDS), None, None), None);
        // Snoozed: nothing until the snooze ends, then the reminder is back.
        assert_eq!(r(None, None, None, Some(NOW + 1)), None);
        assert_eq!(
            r(None, None, None, Some(NOW)),
            Some(ReminderReason::NoFolder)
        );
    }

    #[test]
    fn unknown_steps_start_onboarding_again() {
        assert_eq!(step_of(None), "data");
        assert_eq!(step_of(Some("review".into())), "review");
        assert_eq!(step_of(Some("skip-everything".into())), "data");
    }
}
