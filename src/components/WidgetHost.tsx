import React, { useState, useEffect, useRef, useMemo } from 'react';
import { listen, emit } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow';
import { getCurrentWindow, currentMonitor } from '@tauri-apps/api/window';
import { LogicalSize, LogicalPosition } from '@tauri-apps/api/dpi';
import { HardwareState, PowerMode } from '../types/hardware';
import { auditPluginAction, createAuditedInvoke } from '../plugins/pluginAudit';
import { Flame, Fan, X, RotateCcw } from 'lucide-react';

interface WidgetHostProps {
  pluginId: string;
}

export const WidgetHost: React.FC<WidgetHostProps> = ({ pluginId }) => {
  const [customHtml, setCustomHtml] = useState<string | null>(null);
  const [hasCustomPlugin, setHasCustomPlugin] = useState(false);
  const [hoverControls, setHoverControls] = useState(false);

  // Fallback demo state
  const [hardware, setHardware] = useState<Partial<HardwareState>>({
    cpu_temp: 45,
    cpu_power_w: 35,
    cpu_fan_rpm: 2200,
    gpu_temp: 42,
    power_mode: 2,
    fan_boost: false,
  });
  const [togglingBoost, setTogglingBoost] = useState(false);
  const iframeRef = useRef<HTMLIFrameElement>(null);

  // 1. Expose Universal Tauri & OpenRevo bridge to iframe
  useEffect(() => {
    (window as any).__OPENREVO_BRIDGE__ = {
      invoke: createAuditedInvoke(pluginId),
      listen: (event: string, handler: any) => listen(event, handler),
      emit: (event: string, payload: any) => emit(event, payload),
      getCurrentWebviewWindow: () => getCurrentWebviewWindow(),
      getCurrentWindow: () => getCurrentWindow(),
      currentMonitor: () => currentMonitor(),
      LogicalSize,
      LogicalPosition,
      startDragging: () => {
        try {
          getCurrentWebviewWindow().startDragging();
        } catch (_) {}
      },
      close: async () => {
        try {
          await invoke('close_custom_widget', { pluginId });
        } catch (_) {}
      },
      pluginId,
    };
  }, [pluginId]);

  // 2. Load custom plugin HTML
  const fetchCustomHtml = async () => {
    try {
      const html = await invoke<string>('load_plugin_html', { pluginId });
      if (html && html.trim().length > 0) {
        setCustomHtml(html);
        setHasCustomPlugin(true);
      } else {
        setHasCustomPlugin(false);
      }
    } catch (err) {
      // If plugin does not contain custom index.html/entry, fallback smoothly to built-in S2 core
      setHasCustomPlugin(false);
    } finally {
      try {
        await invoke('widget_ready', { pluginId });
      } catch (_) {}
    }
  };

  useEffect(() => {
    fetchCustomHtml();
  }, [pluginId]);

  // 3. Fallback heartbeat & telemetry sync
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let timer: any;

    const fetchSnapshot = async () => {
      try {
        const state = await invoke<HardwareState>('get_hardware_status');
        setHardware((prev) => ({ ...prev, ...state }));
        // Broadcast to sandboxed iframe as postMessage backup
        if (iframeRef.current?.contentWindow) {
          iframeRef.current.contentWindow.postMessage(
            { type: 'OPENREVO_TELEMETRY', payload: state },
            '*'
          );
        }
      } catch (_) {}
    };

    const init = async () => {
      await fetchSnapshot();
      try {
        unlisten = await listen<HardwareState>('hardware-state-updated', (event) => {
          setHardware((prev) => ({ ...prev, ...event.payload }));
          if (iframeRef.current?.contentWindow) {
            iframeRef.current.contentWindow.postMessage(
              { type: 'OPENREVO_TELEMETRY', payload: event.payload },
              '*'
            );
          }
        });
      } catch (_) {}
    };

    init();
    timer = setInterval(fetchSnapshot, 1500);

    return () => {
      if (timer) clearInterval(timer);
      if (unlisten) unlisten();
    };
  }, [pluginId]);

  // 4. Listen to postMessage from sandboxed iframe
  useEffect(() => {
    const handleMessage = async (e: MessageEvent) => {
      // Security: Only accept messages from this host's own plugin iframe
      if (!iframeRef.current || e.source !== iframeRef.current.contentWindow) return;
      if (!e.data || typeof e.data !== 'object') return;
      const { action, payload } = e.data;
      auditPluginAction(pluginId, action);
      try {
        switch (action) {
          case 'START_DRAGGING':
            getCurrentWebviewWindow().startDragging();
            break;
          case 'SET_POWER_MODE':
            if (payload?.mode) {
              await invoke('set_power_mode', { mode: payload.mode as PowerMode });
            }
            break;
          case 'SET_FAN_BOOST':
            if (typeof payload?.enable === 'boolean') {
              await invoke('set_fan_boost', { enable: payload.enable });
            }
            break;
          case 'CLOSE_WIDGET':
            await invoke('close_custom_widget', { pluginId });
            break;
          default:
            break;
        }
      } catch (_) {}
    };

    window.addEventListener('message', handleMessage);
    return () => window.removeEventListener('message', handleMessage);
  }, [pluginId]);

  // 5. Injected runtime bridge scripts & Import Map for custom HTML
  const preparedHtml = useMemo(() => {
    if (!customHtml) return '';

    const bridgeScript = `
<script>
(function() {
  var b = window.parent.__OPENREVO_BRIDGE__ || {};
  window.__OPENREVO_WIDGET_ID__ = "${pluginId}";
  window.OpenRevo = {
    invoke: b.invoke || function() { return Promise.reject("Bridge unavailable"); },
    listen: b.listen || function() { return Promise.resolve(function() {}); },
    emit: b.emit || function() { return Promise.resolve(); },
    startDragging: b.startDragging || function() {},
    close: b.close || function() {},
    pluginId: "${pluginId}"
  };
  window.__TAURI__ = {
    core: { invoke: b.invoke },
    event: { listen: b.listen, emit: b.emit },
    webviewWindow: { getCurrentWebviewWindow: b.getCurrentWebviewWindow },
    window: { getCurrentWindow: b.getCurrentWindow, currentMonitor: b.currentMonitor },
    dpi: { LogicalSize: b.LogicalSize, LogicalPosition: b.LogicalPosition }
  };

  // Support data-tauri-drag-region natively inside iframe
  document.addEventListener('pointerdown', function(e) {
    if (e.button !== 0) return;
    var target = e.target;
    if (target && target.closest('[data-tauri-drag-region]')) {
      if (!target.closest('button, input, select, textarea, [data-no-drag]')) {
        if (b.startDragging) b.startDragging();
      }
    }
  });

  // Listen for fallback postMessage telemetry
  window.addEventListener('message', function(e) {
    if (e.data && e.data.type === 'OPENREVO_TELEMETRY') {
      window.dispatchEvent(new CustomEvent('openrevo-telemetry', { detail: e.data.payload }));
    }
  });
})();
</script>
<script type="importmap">
{
  "imports": {
    "@tauri-apps/api/core": "data:text/javascript,export const invoke = window.parent.__OPENREVO_BRIDGE__.invoke;",
    "@tauri-apps/api/event": "data:text/javascript,export const listen = window.parent.__OPENREVO_BRIDGE__.listen; export const emit = window.parent.__OPENREVO_BRIDGE__.emit;",
    "@tauri-apps/api/webviewWindow": "data:text/javascript,export const getCurrentWebviewWindow = window.parent.__OPENREVO_BRIDGE__.getCurrentWebviewWindow;",
    "@tauri-apps/api/window": "data:text/javascript,export const getCurrentWindow = window.parent.__OPENREVO_BRIDGE__.getCurrentWindow; export const currentMonitor = window.parent.__OPENREVO_BRIDGE__.currentMonitor;",
    "@tauri-apps/api/dpi": "data:text/javascript,export const LogicalSize = window.parent.__OPENREVO_BRIDGE__.LogicalSize; export const LogicalPosition = window.parent.__OPENREVO_BRIDGE__.LogicalPosition;"
  }
}
</script>
`;

    // Inject into head, or prepend if no head
    if (customHtml.includes('<head>')) {
      return customHtml.replace('<head>', `<head>${bridgeScript}`);
    } else if (customHtml.includes('<html>')) {
      return customHtml.replace('<html>', `<html><head>${bridgeScript}</head>`);
    } else {
      return `${bridgeScript}${customHtml}`;
    }
  }, [customHtml, pluginId]);

  const handleClose = async (e: React.MouseEvent) => {
    e.stopPropagation();
    try {
      await invoke('close_custom_widget', { pluginId });
    } catch (_) {}
  };

  const handlePointerDown = (e: React.PointerEvent) => {
    if (e.button !== 0) return;
    const target = e.target as HTMLElement | null;
    if (target && target.closest('button, [data-no-drag="true"]')) {
      return;
    }
    try {
      getCurrentWebviewWindow().startDragging();
    } catch (_) {}
  };

  const handleToggleFanBoost = async (e: React.MouseEvent) => {
    e.stopPropagation();
    if (togglingBoost) return;
    setTogglingBoost(true);
    const nextVal = !hardware.fan_boost;
    setHardware((prev) => ({ ...prev, fan_boost: nextVal }));
    try {
      await invoke('set_fan_boost', { enable: nextVal });
    } catch (_) {
      setHardware((prev) => ({ ...prev, fan_boost: !nextVal }));
    } finally {
      setTogglingBoost(false);
    }
  };

  const handleCyclePowerMode = async (e: React.MouseEvent) => {
    e.stopPropagation();
    const cur = hardware.power_mode ?? 2;
    const nextMode: PowerMode = cur === 1 ? 2 : cur === 2 ? 3 : 1;
    setHardware((prev) => ({ ...prev, power_mode: nextMode }));
    try {
      await invoke('set_power_mode', { mode: nextMode });
    } catch (_) {}
  };

  // If user has custom HTML, render the custom iframe
  if (hasCustomPlugin && preparedHtml) {
    return (
      <div
        onMouseEnter={() => setHoverControls(true)}
        onMouseLeave={() => setHoverControls(false)}
        style={{
          width: '100vw',
          height: '100vh',
          background: 'transparent',
          position: 'relative',
          overflow: 'hidden',
          userSelect: 'none',
        }}
      >
        <iframe
          ref={iframeRef}
          srcDoc={preparedHtml}
          title={`widget-${pluginId}`}
          sandbox="allow-scripts allow-forms allow-same-origin allow-modals"
          style={{
            width: '100%',
            height: '100%',
            border: 'none',
            background: 'transparent',
            display: 'block',
          }}
          onLoad={() => {
            try {
              invoke('widget_ready', { pluginId });
            } catch (_) {}
          }}
        />

        {/* Discreet rescue & control HUD on hover */}
        {hoverControls && (
          <div
            style={{
              position: 'absolute',
              top: '4px',
              right: '4px',
              display: 'flex',
              gap: '4px',
              zIndex: 99999,
              background: 'rgba(15, 23, 42, 0.75)',
              backdropFilter: 'blur(8px)',
              borderRadius: '6px',
              padding: '2px 4px',
              border: '1px solid rgba(255, 255, 255, 0.1)',
            }}
          >
            <button
              onClick={fetchCustomHtml}
              title="重新加载挂件代码"
              style={{
                background: 'transparent',
                border: 'none',
                color: 'rgba(255, 255, 255, 0.6)',
                cursor: 'pointer',
                padding: '2px',
                display: 'flex',
                alignItems: 'center',
                justifyContent: 'center',
                borderRadius: '4px',
              }}
            >
              <RotateCcw size={11} />
            </button>
            <button
              onClick={handleClose}
              title="关闭悬浮挂件"
              style={{
                background: 'transparent',
                border: 'none',
                color: 'rgba(255, 255, 255, 0.6)',
                cursor: 'pointer',
                padding: '2px',
                display: 'flex',
                alignItems: 'center',
                justifyContent: 'center',
                borderRadius: '4px',
              }}
            >
              <X size={11} />
            </button>
          </div>
        )}
      </div>
    );
  }

  // Fallback: Built-in S2 Energy Core Capsule Demo
  const cpuTemp = hardware.cpu_temp ?? 45;
  const isOverheat = cpuTemp > 80;
  const isWarm = cpuTemp > 68;
  const coreGlow = isOverheat ? '#ef4444' : isWarm ? '#f59e0b' : '#38bdf8';
  const modeName = hardware.power_mode === 1 ? '办公' : hardware.power_mode === 3 ? '狂暴' : '均衡';

  return (
    <div
      style={{
        width: '100vw',
        height: '100vh',
        background: 'transparent',
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        overflow: 'hidden',
        userSelect: 'none',
        padding: '8px',
        boxSizing: 'border-box',
      }}
    >
      {/* Organic Holographic Widget Capsule */}
      <div
        onPointerDown={handlePointerDown}
        style={{
          width: '100%',
          height: '100%',
          borderRadius: '24px',
          background: 'rgba(15, 23, 42, 0.82)',
          backdropFilter: 'blur(16px)',
          border: `1.5px solid ${hardware.fan_boost ? '#ef4444' : 'rgba(255, 255, 255, 0.12)'}`,
          boxShadow: `0 8px 32px rgba(0, 0, 0, 0.5), 0 0 24px ${coreGlow}33`,
          display: 'flex',
          flexDirection: 'column',
          alignItems: 'center',
          justifyContent: 'space-between',
          padding: '12px 14px',
          boxSizing: 'border-box',
          position: 'relative',
          transition: 'border-color 0.3s, box-shadow 0.3s',
          cursor: 'grab',
        }}
      >
        {/* Top Header Controls */}
        <div
          style={{
            width: '100%',
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'space-between',
          }}
        >
          <div style={{ display: 'flex', alignItems: 'center', gap: '6px' }}>
            <span
              style={{
                width: '7px',
                height: '7px',
                borderRadius: '50%',
                background: coreGlow,
                boxShadow: `0 0 8px ${coreGlow}`,
              }}
            />
            <span
              style={{
                fontSize: '11px',
                fontWeight: 700,
                letterSpacing: '0.05em',
                color: 'rgba(255, 255, 255, 0.85)',
              }}
            >
              {pluginId.toUpperCase()}
            </span>
          </div>

          <button
            onClick={handleClose}
            title="关闭挂件"
            style={{
              background: 'transparent',
              border: 'none',
              color: 'rgba(255, 255, 255, 0.4)',
              cursor: 'pointer',
              padding: '2px',
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              borderRadius: '4px',
            }}
          >
            <X size={13} />
          </button>
        </div>

        {/* Central S2 Energy Core Sensor */}
        <div
          style={{
            display: 'flex',
            flexDirection: 'column',
            alignItems: 'center',
            gap: '2px',
            margin: '4px 0',
          }}
        >
          <div
            style={{
              fontSize: '34px',
              fontWeight: 900,
              fontFamily: 'monospace',
              color: '#ffffff',
              lineHeight: 1,
              textShadow: `0 0 16px ${coreGlow}`,
            }}
          >
            {cpuTemp}°
          </div>
          <div
            style={{
              fontSize: '10px',
              color: 'rgba(255, 255, 255, 0.5)',
              letterSpacing: '0.05em',
            }}
          >
            {hardware.cpu_power_w ? `${Math.round(hardware.cpu_power_w)}W` : 'CPU 结温'}
            {' · '}
            {hardware.cpu_fan_rpm ? `${hardware.cpu_fan_rpm} RPM` : '风扇'}
          </div>
        </div>

        {/* Action Triggers: Power Mode Cycle & Fan Boost */}
        <div
          style={{
            width: '100%',
            display: 'flex',
            gap: '6px',
          }}
        >
          <button
            onClick={handleCyclePowerMode}
            title="点击轮转性能模式"
            style={{
              flex: 1,
              height: '26px',
              borderRadius: '8px',
              background: 'rgba(255, 255, 255, 0.08)',
              border: '1px solid rgba(255, 255, 255, 0.1)',
              color: '#fff',
              fontSize: '11px',
              fontWeight: 600,
              cursor: 'pointer',
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              gap: '4px',
            }}
          >
            <Flame size={12} color={coreGlow} />
            <span>{modeName}</span>
          </button>

          <button
            onClick={handleToggleFanBoost}
            title="一键全速强冷 (Fan Boost)"
            style={{
              flex: 1,
              height: '26px',
              borderRadius: '8px',
              background: hardware.fan_boost ? '#ef4444' : 'rgba(255, 255, 255, 0.08)',
              border: hardware.fan_boost ? '1px solid #ef4444' : '1px solid rgba(255, 255, 255, 0.1)',
              color: '#fff',
              fontSize: '11px',
              fontWeight: 600,
              cursor: 'pointer',
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              gap: '4px',
              boxShadow: hardware.fan_boost ? '0 0 12px rgba(239, 68, 68, 0.5)' : 'none',
              transition: 'all 0.2s',
            }}
          >
            <Fan size={12} className={hardware.fan_boost ? 'animate-spin' : ''} />
            <span>{hardware.fan_boost ? '强冷中' : '强冷'}</span>
          </button>
        </div>
      </div>
    </div>
  );
};

