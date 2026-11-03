import { invoke } from '@tauri-apps/api/core';

// Transition policy: an undeclared manifest permission only produces a warning for now.
// Calls are not blocked, so existing plugins keep working while authors add declarations.

const COMMAND_PERMISSIONS: Record<string, string> = {
  raw_ec_read: 'hardware:raw_ec_read',
  raw_ec_write: 'hardware:raw_ec_write',
  raw_mailbox_invoke: 'hardware:raw_mailbox',
  set_power_mode: 'power:control',
  set_battery_limit: 'power:control',
  set_battery_mode: 'power:control',
  set_battery_hardware_limit: 'power:control',
  set_fan_boost: 'fan:control',
  start_plugin_service: 'service:control',
  stop_plugin_service: 'service:control',
  install_plugin_from_url: 'plugin:manage',
  uninstall_plugin: 'plugin:manage',
  open_external_url: 'system:open_url',
};

const ACTION_COMMANDS: Record<string, string> = {
  RAW_EC_READ: 'raw_ec_read',
  RAW_EC_WRITE: 'raw_ec_write',
  RAW_MAILBOX_INVOKE: 'raw_mailbox_invoke',
  SET_POWER_MODE: 'set_power_mode',
  SET_BATTERY_LIMIT: 'set_battery_limit',
  SET_FAN_BOOST: 'set_fan_boost',
  START_PLUGIN_SERVICE: 'start_plugin_service',
  STOP_PLUGIN_SERVICE: 'stop_plugin_service',
};

const declaredPermissions = new Map<string, Promise<string[]>>();
const reportedWarnings = new Set<string>();

function getDeclaredPermissions(pluginId: string): Promise<string[]> {
  let declared = declaredPermissions.get(pluginId);
  if (!declared) {
    declared = invoke<{ permissions?: unknown }>('get_plugin_manifest', { id: pluginId })
      .then((manifest) => (Array.isArray(manifest?.permissions) ? (manifest.permissions as string[]) : []))
      .catch(() => []);
    declaredPermissions.set(pluginId, declared);
  }
  return declared;
}

export function auditPluginCommand(pluginId: string, command: string): void {
  const required = COMMAND_PERMISSIONS[command];
  if (!required) return;

  void getDeclaredPermissions(pluginId).then((declared) => {
    const key = `${pluginId}:${command}`;
    if (declared.includes(required) || reportedWarnings.has(key)) return;
    reportedWarnings.add(key);
    console.warn(
      `[PluginAudit] Plugin "${pluginId}" called "${command}" without declaring "${required}" in manifest.json. ` +
        'This is allowed during the transition period and will be blocked in a future release.'
    );
  });
}

export function auditPluginAction(pluginId: string, action: string): void {
  const command = ACTION_COMMANDS[action];
  if (command) auditPluginCommand(pluginId, command);
}

export function createAuditedInvoke(pluginId: string): typeof invoke {
  return ((command: string, args?: Parameters<typeof invoke>[1], options?: Parameters<typeof invoke>[2]) => {
    auditPluginCommand(pluginId, command);
    return invoke(command, args, options);
  }) as typeof invoke;
}
