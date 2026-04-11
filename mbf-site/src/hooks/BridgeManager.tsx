import { AdbServerClient } from "@yume-chan/adb";
import {
  useState,
  useEffect,
  useCallback,
  createContext,
  useContext,
} from "react";
import { Log } from "../Logging";
import { BridgeFactory, IBridge } from "../BridgeFactory";

export interface BridgeManagerData {
  checkedForBridge: boolean;
  bridge: IBridge | null;
  bridgeClient: AdbServerClient | null;
  adbDevices: AdbServerClient.Device[];
  bridgeError: unknown | null;
  scanning: boolean;
  clearDevices: () => void;
}

export interface BridgeManagerComponents {
  /** A component that scans for devices when mounted */
  DeviceScanner: React.FC;

  /** A context provider to provide the bridge data to the component tree */
  BridgeManagerContextProvider: React.FC<React.PropsWithChildren>;
}

const BridgeManagerContext = createContext<Readonly<BridgeManagerData> | null>(null);

/**
 * Data provided by the BridgeManager context.
 *
 * - checkedForBridge: whether an initial check for a running ADB bridge has completed.
 * - bridgeClient: an AdbServerClient instance when a bridge is available, otherwise null.
 * - adbDevices: list of devices reported by the bridge (filtered to state === "device").
 * - bridgeError: last error encountered while interacting with the bridge, if any.
 * - scanning: whether device polling is currently active.
 */
export function useBridgeManager(): Readonly<
  BridgeManagerData & BridgeManagerComponents
> {
  const [checkedForBridge, setCheckedForBridge] = useState(false);
  const [bridge, setBridge] = useState<IBridge | null>(null);
  const [bridgeClient, setBridgeClient] = useState<AdbServerClient | null>(
    null
  );
  const [adbDevices, setAdbDevices] = useState<AdbServerClient.Device[]>([]);
  const [bridgeError, setBridgeError] = useState<unknown | null>(null);
  const [scanning, setScanning] = useState(true);
  const _DeviceScanner = useCallback<React.FC>(
    function DeviceScanner() {
      useEffect(() => {
        setScanning(true);

        return () => setScanning(false);
      }, [bridgeClient]);

      return null;
    },
    [setScanning]
  );
  const clearDevices = useCallback(function clearDevices() {
    setAdbDevices([]);
  }, [setAdbDevices]);
  const _BridgeManagerContextProvider = useCallback<
    React.FC<React.PropsWithChildren>
  >(
    function BridgeManagerContextProvider({ children }) {
      return (
        <BridgeManagerContext.Provider
          value={{
            checkedForBridge,
            bridge,
            bridgeClient,
            adbDevices,
            bridgeError,
            scanning,
            clearDevices,
          }}
        >
          {children}
        </BridgeManagerContext.Provider>
      );
    },
    [checkedForBridge, bridge, bridgeClient, adbDevices, bridgeError, scanning, clearDevices]
  );

  // Check if the bridge is running
  useEffect(() => {
    if (checkedForBridge) return;
    
    const abortController = new AbortController();

    BridgeFactory.getBridge(abortController).then( async (bridge) => {
      if (bridge) {
        setBridge(bridge);
        bridge.getConnector().then(client => {
          if (client) {
            setBridgeClient(new AdbServerClient(client));
          } else {
            setBridgeClient(null);
          }
        });
      }
    }).finally(() => setCheckedForBridge(true));
    
    return () => abortController.abort();
  }, [checkedForBridge]);

  // Monitor available devices using an observer
  useEffect(() => {
    if (!bridgeClient || !scanning) return;

    let observer: AdbServerClient.DeviceObserver | null = null;
    let isStopped = false;

    bridgeClient.trackDevices({ includeStates: ["device"] }).then(obs => {
      if (isStopped) {
        obs.stop();
        return;
      }

      observer = obs;

      obs.onListChange(devices => {
        setAdbDevices([...devices]);
        setBridgeError(null);
      });

      obs.onError(err => {
        setAdbDevices([]);
        setBridgeError(err);
        setScanning(false);
        Log.error("Device observer error: " + err, err);
      });
    }).catch(err => {
      if (!isStopped) {
        setAdbDevices([]);
        setBridgeError(err);
        setScanning(false);
        Log.error("Failed to start device observer: " + err, err);
      }
    });

    return () => {
      isStopped = true;
      observer?.stop();
    };
  }, [bridgeClient, scanning, setAdbDevices, setBridgeError, setScanning]);

  return {
    checkedForBridge,
    bridge,
    bridgeClient,
    adbDevices,
    bridgeError,
    scanning,
    DeviceScanner: _DeviceScanner,
    BridgeManagerContextProvider: _BridgeManagerContextProvider,
    clearDevices
  };
}

export function useBridgeManagerContext() {
  const context = useContext(BridgeManagerContext);

  if (!context) {
    throw new Error(
      "useBridgeManagerContext must be used within a BridgeManagerContextProvider"
    );
  }

  return context;
}
