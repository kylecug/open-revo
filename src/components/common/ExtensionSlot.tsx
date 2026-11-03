import React, { useState, useEffect, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen, emit } from '@tauri-apps/api/event';
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow';
import { getCurrentWindow, currentMonitor, LogicalSize, LogicalPosition } from '@tauri-apps/api/window';
import { CustomPluginItem } from '../PluginsView';
import { createAuditedInvoke } from '../../plugins/pluginAudit';

export interface ExtensionSlotProps {
  name: string;
  className?: string;
  style?: React.CSSProperties;
  fallback?: React.ReactNode;
}

export const ExtensionSlot: React.FC<ExtensionSlotProps> = ({
  name,
  className,
  style,
  fallback = null,
}) => {
  const [matchingPlugin, setMatchingPlugin] = useState<CustomPluginItem | null>(null);
  const [pluginHtml, setPluginHtml] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const iframeRef = useRef<HTMLIFrameElement>(null);

  // 1. Find matched active slot plugin
  const fetchMatchingSlotPlugin = async () => {
    try {
      const plugins = await invoke<CustomPluginItem[]>('get_custom_plugins');
      const match = plugins.find(
        (p) => p.plugin_type === 'slot' && p.target_slot === name && p.enabled && p.is_compatible !== false
      );
      if (match) {
        setMatchingPlugin(match);
        const html = await invoke<string>('load_plugin_html', { pluginId: match.id });
        if (html && html.trim().length > 0) {
          setPluginHtml(html);
        } else {
          setPluginHtml(null);
        }
      } else {
        setMatchingPlugin(null);
        setPluginHtml(null);
      }
    } catch (err) {
      console.warn(`[ExtensionSlot] Failed to query plugin for slot "${name}":`, err);
      setMatchingPlugin(null);
      setPluginHtml(null);
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    fetchMatchingSlotPlugin();

    // Listen to custom plugins toggle or reload events
    let unlisten: (() => void) | undefined;
    listen('plugins-updated', () => {
      fetchMatchingSlotPlugin();
    }).then((un) => {
      unlisten = un;
    });

    return () => {
      if (unlisten) unlisten();
    };
  }, [name]);

  // 2. Setup OpenRevo Bridge for sandboxed iframe
  useEffect(() => {
    if (!matchingPlugin) return;

    (window as any).__OPENREVO_BRIDGE__ = {
      invoke: createAuditedInvoke(matchingPlugin.id),
      listen: (event: string, handler: any) => listen(event, handler),
      emit: (event: string, payload: any) => emit(event, payload),
      getCurrentWebviewWindow: () => getCurrentWebviewWindow(),
      getCurrentWindow: () => getCurrentWindow(),
      currentMonitor: () => currentMonitor(),
      LogicalSize,
      LogicalPosition,
      pluginId: matchingPlugin.id,
      slotName: name,
      registerWatcher: (watcher: any) =>
        invoke('register_custom_watcher', {
          watcher: { ...watcher, plugin_id: matchingPlugin.id },
        }),
      unregisterWatcher: (watcherId: string) =>
        invoke('unregister_custom_watcher', { id: watcherId }),
      getRegisteredWatchers: () => invoke('get_registered_watchers'),
    };
  }, [matchingPlugin, name]);

  if (loading) {
    return null;
  }

  if (!matchingPlugin || !pluginHtml) {
    return fallback ? <>{fallback}</> : null;
  }

  const height = matchingPlugin.slot_height || '54px';

  return (
    <div
      className={`extension-slot-container ${className || ''}`}
      data-slot={name}
      style={{
        width: '100%',
        minHeight: height,
        position: 'relative',
        display: 'flex',
        flexDirection: 'column',
        ...style,
      }}
    >
      <iframe
        ref={iframeRef}
        srcDoc={pluginHtml}
        title={`Slot: ${name}`}
        sandbox="allow-scripts allow-forms allow-same-origin"
        style={{
          width: '100%',
          height: height,
          border: 'none',
          background: 'transparent',
          overflow: 'hidden',
          display: 'block',
        }}
      />
    </div>
  );
};
