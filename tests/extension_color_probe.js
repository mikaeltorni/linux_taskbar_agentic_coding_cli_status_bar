const DEFAULTS = {codex: true, claude_code: false, grok: true, opencode_go: false, openrouter: false, cursor: false};
const GLib = imports.gi.GLib;
const NOW = 1783158600;
const launchCalls = [];
const logCalls = [];
const Gio = {AppInfo: {
  launch_default_for_uri_async(uri, context, cancellable, callback) {
    launchCalls.push(['launch', uri, context, cancellable]);
    callback(null, {uri});
  },
  launch_default_for_uri_finish(result) {
    launchCalls.push(['finish', result.uri]);
  },
}};
function log(level, message) {
  logCalls.push([level, message]);
}
const validForecastLaunch = openExternalUrl(
  'https://polymarket.com/', 'Polymarket');
const referenceForecastLaunches = [
  ['https://whenreset.dev/codex', 'WhenReset'],
  ['https://codex-resets.com/', 'Codex Resets'],
  ['https://willcodexreset.org/', 'Will Codex Reset'],
].map(([url, name]) => openExternalUrl(url, name));
const rejectedForecastLaunch = openExternalUrl(
  'javascript:alert(1)', 'Unsafe');
const results = {
  missing_forecast_volume: formatForecastVolume(undefined),
  invalid_forecast_volume: formatForecastVolume(-5),
  clutter_markup_prefix: CLUTTER_MARKUP_PREFIX,
  color_100: percentColor(100),
  color_1: percentColor(1),
  color_0: percentColor(0),
  color_50: percentColor(50),
  color_99: percentColor(99),
  color_2: percentColor(2),
  color_26: percentColor(26),
  color_48: percentColor(48),
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
  team_instance_heading: cliSectionHeading(
    codexInstanceTitle({id: 1, plan: 'team', resets_available: 2}),
    {plan: 'team', resets_available: 2}),
  personal_instance_heading: cliSectionHeading(
    codexInstanceTitle({id: 2, label: 'main', resets_available: 1}),
    {plan: 'team', resets_available: 1}),
  missing_instance_heading: cliSectionHeading(
    codexInstanceTitle({id: 1}), {plan: 'team'}),
  zero_instance_heading: cliSectionHeading(
    codexInstanceTitle({id: 1}), {plan: 'team', resets_available: 0}),
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
  luna_reserve_on_cx3: codexBarMarkup('CX', {
    selected: 3,
    instances: [
      {id: 1, windows: [
        {label: '5h', used_percent: 20},
        {label: 'week', used_percent: 10}]},
      {id: 3, windows: [
        {label: '5h', used_percent: 100},
        {label: 'week', used_percent: 32},
        {label: 'luna_reserve', used_percent: 8}]},
    ]
  }, NOW),
  luna_reserve_direct_on_selected_cx3: codexBarMarkup('CX', {
    selected: 3,
    windows: [
      {label: 'week', used_percent: 32},
      {label: 'luna_reserve', used_percent: 8},
    ],
    instances: [
      {id: 1, windows: [{label: 'week', used_percent: 10}]},
      {id: 3, windows: [{label: 'week', used_percent: 32}]},
    ],
  }, NOW),
  depleted_luna_reserve_markup: lunaReserveMarkup(
    {label: 'luna_reserve', used_percent: 100}, NOW),
  luna_reserve_window_title: codexWindowTitle({label: 'luna_reserve'}),
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
  codex_with_forecast: codexBarWithForecastMarkup(
    {enabled: true, consensus_percent: 64.4},
    'CX',
    {selected: 1, windows: [{label: 'week', used_percent: 43}]},
    NOW),
  codex_with_stale_forecast: codexBarWithForecastMarkup(
    {enabled: true, consensus_percent: 64.4, stale: true},
    'CX',
    {selected: 1, windows: [{label: 'week', used_percent: 43}]},
    NOW),
  codex_without_forecast: codexBarWithForecastMarkup(
    {enabled: false, consensus_percent: 64.4},
    'CX',
    {selected: 1, windows: [{label: 'week', used_percent: 43}]},
    NOW),
  codex_without_consensus: codexBarWithForecastMarkup(
    {enabled: true, consensus_percent: null},
    'CX',
    {selected: 1, windows: [{label: 'week', used_percent: 43}]},
    NOW),
  codex_with_24_48_forecast: codexBarWithForecastMarkup(
    {
      enabled: true,
      consensus_percent: 26,
      consensus_percent_24h: 26,
      consensus_percent_48h: 48,
    },
    'CX',
    {selected: 1, windows: [{label: 'week', used_percent: 43}]},
    NOW),
  codex_with_only_48h_forecast: codexBarWithForecastMarkup(
    {
      enabled: true,
      horizon: '48h',
      consensus_percent: 48,
      consensus_percent_48h: 48,
    },
    'CX',
    {selected: 1, windows: [{label: 'week', used_percent: 43}]},
    NOW),
  forecast_menu_entries: codexResetForecastMenuEntries({
    enabled: true,
    horizon: '24h',
    consensus_percent: 26,
    sites: [
      {name: 'codex-reset.com', url: 'https://codex-reset.com/', predictions: [
        {label: '24h', percent: 24}, {label: '48h', percent: 42}]},
      {id: 'polymarket', name: 'Polymarket', url: 'https://polymarket.com/', predictions: [
        {label: 'September 23', percent: 24, volume_usd: 1234.56},
        {label: 'September 24', percent: 15, volume_usd: 0}]},
      {name: 'Unavailable tracker', url: 'https://example.com/', predictions: [], error: 'HTTP 503'},
      {name: 'WhenReset', url: 'https://whenreset.dev/codex',
        reference: true, detail: 'latest banked reset · Sep 22 18:23 UTC', predictions: []},
      {name: 'Codex Resets', url: 'https://codex-resets.com/',
        reference: true, detail: 'latest banked Sep 22 18:23 · direct scheduled Sep 23 06:59 UTC', predictions: []},
      {name: 'Will Codex Reset', url: 'https://willcodexreset.org/',
        reference: true, detail: '24h 20% · 48h 35% · mirrored outlook', predictions: []},
    ],
  }),
  forecast_menu_weekly_questions: codexResetForecastMenuEntries({
    enabled: true,
    horizon: '24h',
    sites: [{id: 'polymarket', name: 'Polymarket', url: 'https://polymarket.com/', predictions: [
      {label: 'Will there be no Codex weekly usage limit reset in the week of September 21', percent: 21},
      {label: 'Will there be exactly 1 Codex weekly usage limit reset in the week of September 21', percent: 14},
    ]}],
  }),
  forecast_menu_count_market: codexResetForecastMenuEntries({
    enabled: true,
    horizon: '24h',
    sites: [{id: 'polymarket', name: 'Polymarket', url: 'https://polymarket.com/', predictions: [
      {label: 'at least 1 reset', percent: 86},
    ]}],
  }),
  forecast_menu_no_open_market: codexResetForecastMenuEntries({
    enabled: true,
    sites: [{id: 'polymarket', name: 'Polymarket', url: 'https://polymarket.com/',
      detail: 'no open market', predictions: []}],
  }),
  forecast_menu_stale_market: codexResetForecastMenuEntries({
    enabled: true,
    sites: [{id: 'polymarket', name: 'Polymarket', url: 'https://polymarket.com/',
      error: 'network timeout', predictions: [{label: 'September 23', percent: 57}]}],
  }),
  disabled_forecast_menu_entries: codexResetForecastMenuEntries({
    enabled: false,
    horizon: '24h',
    consensus_percent: 26,
  }),
  forecast_menu_24_48: codexResetForecastMenuEntries({
    enabled: true,
    horizon: '24h',
    consensus_percent: 26,
    consensus_percent_24h: 26,
    consensus_percent_48h: 48,
    sites: [
      {name: 'codex-reset.com', url: 'https://codex-reset.com/', predictions: [
        {label: '24h', percent: 24}, {label: '48h', percent: 42}]},
    ],
  }),
  forecast_menu_only_48h: codexResetForecastMenuEntries({
    enabled: true,
    horizon: '48h',
    consensus_percent: 48,
    consensus_percent_48h: 48,
  }),
  forecast_color_100_0: codexResetForecastMarkup({
    enabled: true,
    consensus_percent_24h: 100,
    consensus_percent_48h: 0,
  }),
  forecast_color_0_100: codexResetForecastMarkup({
    enabled: true,
    consensus_percent_24h: 0,
    consensus_percent_48h: 100,
  }),
  valid_forecast_launch: validForecastLaunch,
  reference_forecast_launches: referenceForecastLaunches,
  rejected_forecast_launch: rejectedForecastLaunch,
  forecast_launch_calls: launchCalls,
  forecast_launch_logs: logCalls,
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
