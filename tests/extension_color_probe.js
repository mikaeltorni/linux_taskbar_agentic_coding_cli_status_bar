const DEFAULTS = {codex: true, claude_code: false, grok: true, opencode_go: false, openrouter: false, cursor: false};
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
  credit_block_team: creditDetailText({
    has_credits: false, unlimited: false, balance: null,
    reached_type: 'workspace_member_credits_depleted', blocked: true}),
  credit_block_generic: creditDetailText({
    has_credits: false, unlimited: false, balance: '0',
    reached_type: 'user_credits_depleted', blocked: true}),
  credit_balance_only: creditDetailText({
    has_credits: false, unlimited: false, balance: '0',
    reached_type: null, blocked: false}),
  credit_zero_formatted: creditDetailText({
    has_credits: false, unlimited: false, balance: '$0.00',
    reached_type: null, blocked: false}),
  // An account that really does carry credits keeps reporting a zero balance.
  credit_zero_with_facility: creditDetailText({
    has_credits: true, unlimited: false, balance: '0',
    reached_type: null, blocked: false}),
  // A non-zero balance is still worth saying regardless of the facility flag.
  credit_nonzero_without_facility: creditDetailText({
    has_credits: false, unlimited: false, balance: '12.50',
    reached_type: null, blocked: false}),
  credit_unlimited: creditDetailText({unlimited: true, balance: null}),
  credit_absent: creditDetailText(null),
  credit_blocked_true: creditBlocked({blocked: true}),
  credit_blocked_false: creditBlocked({blocked: false, balance: '0'}),
  // A healthy-looking window set still flags the credit block.
  credit_marked_segment: segmentMarkup('CX1', {windows: [
    {label: '5h', used_percent: 20, resets_at: NOW + 3600},
    {label: 'week', used_percent: 10, resets_at: 1783617000}],
    credits: {blocked: true, reached_type: 'workspace_member_credits_depleted'}
  }, NOW),
  credit_unmarked_segment: segmentMarkup('CX1', {windows: [
    {label: '5h', used_percent: 20, resets_at: NOW + 3600},
    {label: 'week', used_percent: 10, resets_at: 1783617000}],
    credits: {blocked: false, balance: '0'}
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
  cursor_segment: cursorBarMarkup('CRS', {windows: [
    {label: 'cursor_models', used_percent: 23.1, resets_at: 1791763200},
    {label: 'other_models', used_percent: 45.0, resets_at: 1791763200},
    {label: 'grok_bot', used_percent: 12.0, resets_at: NOW + 259200}],
    usage_limit_reset: 1791763200,
    included_fields: [
      {label: 'cursor_models remaining', text: '$15.38 of $20.00'},
      {label: 'onDemandUsage', text: '$999.00'}],
  }, NOW),
  cursor_submenu_extras: cursorExtraDetailTexts({
    usage_limit_reset: 1791763200,
    included_fields: [
      {label: 'cursor_models remaining', text: '$15.38 of $20.00'},
      {label: 'usage limit reset', text: 'resets 14 Oct'},
      {label: 'onDemandUsage', text: '$999.00'}],
  }),
  cursor_on_demand_absent: cursorBarMarkup('CRS', {windows: [
    {label: 'cursor_models', used_percent: 23.1, resets_at: 1791763200},
    {label: 'other_models', used_percent: 45.0, resets_at: 1791763200},
    {label: 'grok_bot', used_percent: 12.0, resets_at: NOW + 259200}],
    usage_limit_reset: 1791763200,
    included_fields: [{label: 'on-demand usage', text: '99.9%'}],
    onDemandUsage: {usedPercent: 99.9},
  }, NOW),
  two_codex_accounts: codexBarMarkup('CX', {
    selected: 2,
    instances: [
      {id: 1, windows: [
        {label: '5h', used_percent: 57, resets_at: NOW + 3600},
        {label: 'week', used_percent: 43, resets_at: NOW + 100000}]},
      {id: 2, label: 'trial', selected: true, windows: [
        {label: 'month', used_percent: 12, resets_at: NOW + 2588400}]},
    ]
  }, NOW),
  first_codex_account: codexBarMarkup('CX', {
    selected: 1,
    instances: [
      {id: 1, windows: [
        {label: '5h', used_percent: 57, resets_at: NOW + 3600},
        {label: 'week', used_percent: 43, resets_at: NOW + 100000}]},
      {id: 2, label: 'trial', windows: [
        {label: 'month', used_percent: 12, resets_at: NOW + 2588400}]},
    ]
  }, NOW),
  second_codex_title: codexInstanceTitle({id: 2, label: 'trial', selected: true}),
  primary_codex_title: codexInstanceTitle({id: 1, label: ''}),
  instance_tag_first: instanceTagColor(1, 3),
  instance_tag_mid: instanceTagColor(2, 3),
  instance_tag_last: instanceTagColor(3, 3),
  instance_tag_only: instanceTagColor(1, 1),
  cold_start_defaults: cliEnabledFromConfig(null, DEFAULTS),
  cold_start_opt_in: cliEnabledFromConfig(
    {clis: {claude_code: {enabled: true}, openrouter: {enabled: true}, opencode_go: {enabled: true}, cursor: {enabled: true}}}, DEFAULTS),
  cold_start_opt_out: cliEnabledFromConfig(
    {clis: {grok: {enabled: false}}}, DEFAULTS),
  cold_start_garbage: cliEnabledFromConfig({clis: {openrouter: {enabled: 'yes'}}}, DEFAULTS),
};
print(JSON.stringify(results));
