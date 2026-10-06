import { describe, expect, it } from 'vitest';
import { driverSnippets, planMove, planSpec, specForStyle, MOTION_STYLES } from '../src/index';

describe('api', () => {
  it('the arc styles are specs', () => {
    const req = { from: { x: 80, y: 640 }, to: { x: 900, y: 120 } };
    for (const style of ['signature_arc', 'spring_settle', 'comet_swoop'] as const) {
      const spec = specForStyle(style, { arcSize: 0.4 })!;
      expect(planSpec(spec, req).samples).toEqual(planMove({ style, arcSize: 0.4 }, req).samples);
    }
    expect(specForStyle('magnetic')).toBeNull();
  });

  it('every style lands on the target', () => {
    for (const style of MOTION_STYLES) {
      const traj = planMove({ style }, { from: { x: 0, y: 0 }, to: { x: 500, y: 300 } });
      expect(traj.end()).toMatchObject({ x: 500, y: 300 });
      expect(traj.arrivalT).toBeLessThanOrEqual(traj.duration());
    }
  });

  it('exports the Cua Driver equivalent', () => {
    const s = driverSnippets(
      { style: 'comet_swoop', timing: 'fitts', arcSize: 0.4, effects: { glow: true } },
      'demo'
    );
    expect(s.configSet).toEqual([
      'cua-driver config set cursor.motion.style comet_swoop',
      'cua-driver config set cursor.motion.timing fitts',
      'cua-driver config set cursor.motion.effects.glow true',
    ]);
    expect(s.cursorMotion).toBe(
      'cua-driver cursor motion --session demo --style comet_swoop --timing fitts --effects glow=on'
    );
    expect(s.setAgentCursorMotion).toEqual({
      session: 'demo',
      style: 'comet_swoop',
      timing: 'fitts',
      arc_size: 0.4,
      effects: { glow: true },
    });
    expect(s.callOnly).toEqual(['arc_size']);
  });
});
