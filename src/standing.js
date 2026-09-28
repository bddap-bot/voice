import * as THREE from 'three';
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

export function anchorStandingIdle(clip, vrm) {
  const pose = standingPose(vrm);
  const tracks = new Map(clip.tracks.map(track => [track.name, track]));
  for (const [bone, { rotation }] of Object.entries(pose)) {
    const node = vrm.humanoid.getNormalizedBoneNode(bone);
    if (!node) continue;
    const track = tracks.get(`${node.name}.quaternion`) ?? tracks.get(`${node.uuid}.quaternion`);
    if (track) {
      const anchor = new THREE.Quaternion().fromArray(rotation).multiply(new THREE.Quaternion().fromArray(track.values).invert());
      const sample = new THREE.Quaternion();
      for (let i = 0; i < track.values.length; i += 4) sample.fromArray(track.values, i).premultiply(anchor).normalize().toArray(track.values, i);
    } else clip.tracks.push(new THREE.QuaternionKeyframeTrack(`${node.name}.quaternion`, [0, clip.duration], [...rotation, ...rotation]));
  }
  const hips = vrm.humanoid.getNormalizedBoneNode('hips');
  const position = tracks.get(`${hips.name}.position`) ?? tracks.get(`${hips.uuid}.position`);
  if (position) {
    const rest = vrm.humanoid.normalizedRestPose.hips.position;
    const offset = rest.map((value, axis) => value - position.values[axis]);
    for (let i = 0; i < position.values.length; i++) position.values[i] += offset[i % 3];
  }
  clip.userData.poseTracks = clip.tracks.map(track => ({ name: track.name, valueSize: track.getValueSize(), interpolant: track.createInterpolant() }));
  return clip;
}
