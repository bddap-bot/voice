import standing from '../docs/poses/standing.json' with { type: 'json' };

export function standingPose(vrm) {
  const pose = {};
  for (const [name, { rotation }] of Object.entries(standing.data)) {
    const bone = name.replace(/ThumbProximal$/, 'ThumbMetacarpal').replace(/ThumbIntermediate$/, 'ThumbProximal');
    const [x, y, z, w] = rotation;
    pose[bone] = { rotation: vrm.meta.metaVersion === '0' ? [x, y, z, w] : [-x, y, -z, w] };
  }
  return pose;
}
