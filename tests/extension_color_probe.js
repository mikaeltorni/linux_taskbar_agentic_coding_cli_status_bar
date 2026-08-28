const GLib = imports.gi.GLib;
const NOW = 1783158600;
const results = {
  color_100: percentColor(100),
  color_1: percentColor(1),
  color_0: percentColor(0),
  color_50: percentColor(50),
  color_99: percentColor(99),
  color_2: percentColor(2),
  left_clamped: percentLeft({used_percent: 130}),
  dash: segmentMarkup('CX', {windows: []}),
  errored_stale_segment: segmentMarkup('CC', {
    error: 'HTTP 429', stale: true, windows: [
      {label: '5h', used_percent: 40, resets_at: NOW + 3600},
      {label: 'week', used_percent: 14, resets_at: NOW + 100000}]
  }, NOW),
  segment: segmentMarkup('CC', {windows: [
    {used_percent: 0}, {used_percent: 99}, {used_percent: 100}]}),
  weekly_exhausted_grok: segmentMarkup('GK', {windows: [
    {label: 'week', used_percent: 100, resets_at: 1783617000}]
  }, NOW),
  weekly_exhausted_codex: segmentMarkup('CX', {windows: [
    {label: '5h', used_percent: 100, resets_at: NOW + 17940},
    {label: 'week', used_percent: 100, resets_at: 1783617000}]
  }, NOW),
  five_hour_exhausted_codex: segmentMarkup('CX', {windows: [
    {label: '5h', used_percent: 100, resets_at: NOW + 17940},
    {label: 'week', used_percent: 59, resets_at: 1783617000}]
  }, NOW),
  five_hour_exhausted_claude: segmentMarkup('CC', {windows: [
    {label: '5h', used_percent: 100, resets_at: NOW + 17940},
    {label: 'week', used_percent: 59, resets_at: 1783617000},
    {label: 'fable', used_percent: 59, resets_at: 1783617000}]
  }, NOW),
  countdown_59m: resetCountdown(NOW + 3540, NOW),
  countdown_60m: resetCountdown(NOW + 3600, NOW),
  rounded_reset_clock: resetClock(1783259999),
  rounded_details_reset: resetsText(1783259999),
  grok_details_reset: resetsText(1783577040, 'week'),
  one_reset_available: resetAvailabilityText(1),
  multiple_resets_available: resetAvailabilityText(2),
  missing_resets_available: resetAvailabilityText(undefined),
  zero_resets_available: resetAvailabilityText(0),
  fable_exhausted: segmentMarkup('CC', {windows: [
    {label: '5h', used_percent: 100, resets_at: NOW + 3300},
    {label: 'week', used_percent: 59, resets_at: 1783617000},
    {label: 'fable', used_percent: 100, resets_at: 1783259999}]
  }, NOW),
  reset_passed_left_full: percentLeft(
    {used_percent: 100, resets_at: NOW - 60}, NOW),
  reset_future_left_exhausted: percentLeft(
    {used_percent: 100, resets_at: NOW + 60}, NOW),
  boot_stale_five_hour_replenished: segmentMarkup('CX', {windows: [
    {label: '5h', used_percent: 100, resets_at: NOW - 3600},
    {label: 'week', used_percent: 82, resets_at: NOW + 100000}]
  }, NOW),
  boot_stale_weekly_replenished: segmentMarkup('CX', {windows: [
    {label: '5h', used_percent: 40, resets_at: NOW + 3600},
    {label: 'week', used_percent: 100, resets_at: NOW - 60}]
  }, NOW),
  opencode_go_segment: segmentMarkup('OG', {windows: [
    {label: 'rolling', used_percent: 0, resets_at: NOW + 17820},
    {label: 'week', used_percent: 83, resets_at: NOW + 234000},
    {label: 'month', used_percent: 92, resets_at: NOW + 2588400}]
  }, NOW),
  opencode_go_month_exhausted: segmentMarkup('OG', {windows: [
    {label: 'rolling', used_percent: 0, resets_at: NOW + 17820},
    {label: 'week', used_percent: 83, resets_at: NOW + 234000},
    {label: 'month', used_percent: 100, resets_at: NOW + 2588400}]
  }, NOW),
  monthly_reset_date: resetDate(NOW + 2588400),
  monthly_details_reset: resetsText(NOW + 2588400, 'month'),
  openrouter_segment: segmentMarkup('OR', {windows: [
    {label: 'day', used_percent: 42, resets_at: NOW + 52000}]
  }, NOW),
  two_codex_accounts: codexBarMarkup('CX', {
    instances: [
      {id: 1, windows: [
        {label: '5h', used_percent: 57, resets_at: NOW + 3600},
        {label: 'week', used_percent: 43, resets_at: NOW + 100000}]},
      {id: 2, label: 'trial', windows: [
        {label: 'month', used_percent: 12, resets_at: NOW + 2588400}]},
    ]
  }, NOW),
  second_codex_title: codexInstanceTitle({id: 2, label: 'trial'}),
  primary_codex_title: codexInstanceTitle({id: 1, label: ''}),
};
print(JSON.stringify(results));
