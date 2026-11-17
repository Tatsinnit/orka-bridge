import { useRef, type ComponentProps } from 'react';
import Setup, { type LocalClusterControl } from './Setup';

// Standalone tests model App's already-active Clusters tab without mounting Designer.
export default function SetupHarness(props: ComponentProps<typeof Setup>) {
  const control = useRef<LocalClusterControl>(null);
  return <><button type="button" onClick={() => control.current?.showPlaces()}>Clusters</button><Setup {...props} ref={control} /></>;
}
