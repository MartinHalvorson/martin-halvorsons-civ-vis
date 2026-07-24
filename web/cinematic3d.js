/* CIVVIS cinematic unit models.
 *
 * This is deliberately dependency-free: the strategic map remains one
 * Canvas2D scene, while close cinematic shots can ask this module to project
 * small real 3D meshes into that canvas.  Geometry is depth-sorted per face,
 * lit from a fixed world key, and articulated from the same movement/attack
 * phase as the simulation renderer.  The ordinary vector figures remain the
 * low-zoom and compatibility fallback.
 */
(function installCinematic3D(global) {
  "use strict";

  const FAMILIES = Object.freeze([
    "embarked", "naval", "air", "rotor", "balloon", "drone", "robot",
    "armor", "gun", "siege", "mounted", "religious", "civilian", "infantry",
  ]);
  const FAMILY_SET = new Set(FAMILIES);
  const MELEE = new Set(["warrior", "eagle_warrior", "swordsman", "legion", "man_at_arms"]);
  const FIREARM = new Set(["musketman", "line_infantry", "infantry", "pike_and_shot",
    "ranger", "spec_ops", "machine_gun"]);
  const SPEAR = new Set(["spearman", "pikeman", "hoplite", "pike_and_shot", "at_crew", "modern_at"]);
  const BOW = new Set(["slinger", "archer", "crossbowman", "pitati_archer",
    "crouching_tiger", "skirmisher", "saka_horse_archer", "maryannu_chariot_archer"]);
  const MODERN_SHIPS = new Set(["ironclad", "battleship", "destroyer", "aircraft_carrier",
    "missile_cruiser"]);
  const SUBMARINES = new Set(["submarine", "nuclear_submarine"]);

  const clamp = (v, lo, hi) => Math.max(lo, Math.min(hi, v));
  const add = (a, b) => [a[0] + b[0], a[1] + b[1], a[2] + b[2]];
  const sub = (a, b) => [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
  const mul = (a, k) => [a[0] * k, a[1] * k, a[2] * k];
  const dot = (a, b) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
  const cross = (a, b) => [a[1] * b[2] - a[2] * b[1],
    a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
  const norm = a => {
    const n = Math.hypot(a[0], a[1], a[2]) || 1;
    return [a[0] / n, a[1] / n, a[2] / n];
  };
  const hex = color => {
    const text = String(color || "#82909a");
    const rgb = text.match(/^rgba?\(\s*([\d.]+)\s*,\s*([\d.]+)\s*,\s*([\d.]+)/i);
    if (rgb) return rgb.slice(1, 4).map(Number);
    const raw = text.replace("#", "");
    const h = raw.length === 3 ? raw.split("").map(c => c + c).join("") : raw.slice(0, 6);
    const value = Number.parseInt(h, 16);
    return Number.isFinite(value)
      ? [(value >> 16) & 255, (value >> 8) & 255, value & 255] : [130, 144, 154];
  };
  const tint = (color, amount, alpha = 1) => {
    const [r, g, b] = hex(color);
    const lift = amount > 1 ? 255 * (amount - 1) * .16 : 0;
    const scale = amount > 1 ? 1 : amount;
    return `rgba(${clamp(Math.round(r * scale + lift), 0, 255)},` +
      `${clamp(Math.round(g * scale + lift), 0, 255)},` +
      `${clamp(Math.round(b * scale + lift), 0, 255)},${alpha})`;
  };

  function rotatePoint(p, rx = 0, ry = 0, rz = 0) {
    let [x, y, z] = p;
    let c = Math.cos(rx), s = Math.sin(rx);
    [y, z] = [y * c - z * s, y * s + z * c];
    c = Math.cos(ry); s = Math.sin(ry);
    [x, z] = [x * c + z * s, -x * s + z * c];
    c = Math.cos(rz); s = Math.sin(rz);
    [x, y] = [x * c - y * s, x * s + y * c];
    return [x, y, z];
  }

  class Scene {
    constructor(ctx, options) {
      this.ctx = ctx;
      this.scale = options.scale || 1;
      this.facing = options.facing < 0 ? -1 : 1;
      this.bank = options.bank || 0;
      this.items = [];
      this.light = norm([-0.55 * this.facing, -0.72, 1.1]);
      this.stroke = options.stroke || "rgba(8,12,16,.78)";
    }

    world(p) {
      let q = [p[0] * this.facing, p[1], p[2]];
      if (this.bank) q = rotatePoint(q, 0, this.bank * .22, this.bank * .12);
      return q;
    }

    project(p) {
      const q = this.world(p);
      const perspective = 88 / (88 + q[1]);
      return {
        x: q[0] * this.scale * perspective,
        y: (q[1] * .38 - q[2]) * this.scale * perspective,
        depth: q[1] + q[2] * .012,
        perspective,
      };
    }

    mesh(points, faces, color, alpha = 1) {
      const world = points.map(p => this.world(p));
      const projected = points.map(p => this.project(p));
      for (const face of faces) {
        const vertices = face.map(i => projected[i]);
        if (vertices.length < 3) continue;
        const a = world[face[0]], b = world[face[1]], c = world[face[2]];
        const normal = norm(cross(sub(b, a), sub(c, a)));
        const direct = Math.max(0, dot(normal, this.light));
        const rim = Math.max(0, normal[2]) * .08;
        this.items.push({
          kind: "face", vertices,
          depth: vertices.reduce((sum, p) => sum + p.depth, 0) / vertices.length,
          fill: tint(color, .42 + direct * .72 + rim, alpha),
          shine: direct > .72 ? direct : 0,
        });
      }
    }

    box(center, size, color, rotation = [0, 0, 0]) {
      const [w, d, h] = size, [cx, cy, cz] = center;
      const points = [];
      for (const z of [-h / 2, h / 2]) for (const y of [-d / 2, d / 2])
        for (const x of [-w / 2, w / 2]) {
          const p = rotatePoint([x, y, z], rotation[0], rotation[1], rotation[2]);
          points.push([p[0] + cx, p[1] + cy, p[2] + cz]);
        }
      this.mesh(points, [[0, 1, 3, 2], [4, 6, 7, 5], [0, 4, 5, 1],
        [2, 3, 7, 6], [0, 2, 6, 4], [1, 5, 7, 3]], color);
    }

    wedge(center, size, color, nose = 1) {
      const [w, d, h] = size, [x, y, z] = center;
      const n = nose >= 0 ? d / 2 : -d / 2, t = -n;
      const points = [[-w / 2, t, -h / 2], [w / 2, t, -h / 2],
        [-w / 2, t, h / 2], [w / 2, t, h / 2], [0, n, -h / 3], [0, n, h / 3]]
        .map(p => add(p, [x, y, z]));
      this.mesh(points, [[0, 1, 3, 2], [0, 4, 1], [2, 3, 5],
        [0, 2, 5, 4], [1, 4, 5, 3]], color);
    }

    tube(a, b, radius, color, sides = 7) {
      const axis = norm(sub(b, a));
      const reference = Math.abs(axis[2]) > .88 ? [0, 1, 0] : [0, 0, 1];
      const u = norm(cross(axis, reference)), v = norm(cross(axis, u));
      const points = [];
      for (const end of [a, b]) for (let i = 0; i < sides; i++) {
        const angle = i * Math.PI * 2 / sides;
        points.push(add(end, add(mul(u, Math.cos(angle) * radius),
          mul(v, Math.sin(angle) * radius))));
      }
      const faces = [];
      for (let i = 0; i < sides; i++) faces.push([i, (i + 1) % sides,
        sides + (i + 1) % sides, sides + i]);
      faces.push([...Array(sides).keys()].reverse());
      faces.push([...Array(sides).keys()].map(i => sides + i));
      this.mesh(points, faces, color);
    }

    ellipsoid(center, radii, color, segments = 8, rings = 4) {
      const points = [];
      for (let r = 0; r <= rings; r++) {
        const lat = -Math.PI / 2 + r * Math.PI / rings;
        for (let i = 0; i < segments; i++) {
          const lon = i * Math.PI * 2 / segments;
          points.push([center[0] + Math.cos(lat) * Math.cos(lon) * radii[0],
            center[1] + Math.cos(lat) * Math.sin(lon) * radii[1],
            center[2] + Math.sin(lat) * radii[2]]);
        }
      }
      const faces = [];
      for (let r = 0; r < rings; r++) for (let i = 0; i < segments; i++) {
        const next = (i + 1) % segments;
        faces.push([r * segments + i, r * segments + next,
          (r + 1) * segments + next, (r + 1) * segments + i]);
      }
      this.mesh(points, faces, color);
    }

    polygon(points, color, thickness = .45) {
      const top = points.map(p => [p[0], p[1], p[2] + thickness / 2]);
      const bottom = points.map(p => [p[0], p[1], p[2] - thickness / 2]);
      const vertices = [...bottom, ...top], n = points.length;
      const faces = [[...Array(n).keys()].reverse(), [...Array(n).keys()].map(i => n + i)];
      for (let i = 0; i < n; i++) faces.push([i, (i + 1) % n, n + (i + 1) % n, n + i]);
      this.mesh(vertices, faces, color);
    }

    glow(point, radius, color, alpha = .7) {
      const p = this.project(point);
      this.items.push({kind: "glow", ...p, radius: radius * this.scale * p.perspective,
        color, alpha});
    }

    shadow(rx, ry, alpha = .28) {
      const p = this.project([0, 0, .1]);
      this.items.push({kind: "shadow", ...p, depth:-1e6, rx: rx * this.scale,
        ry: ry * this.scale, alpha});
    }

    flush() {
      const ctx = this.ctx;
      this.items.sort((a, b) => a.depth - b.depth);
      ctx.save();
      ctx.lineJoin = "round"; ctx.lineCap = "round";
      for (const item of this.items) {
        if (item.kind === "face") {
          ctx.beginPath();
          ctx.moveTo(item.vertices[0].x, item.vertices[0].y);
          for (let i = 1; i < item.vertices.length; i++)
            ctx.lineTo(item.vertices[i].x, item.vertices[i].y);
          ctx.closePath(); ctx.fillStyle = item.fill; ctx.fill();
          ctx.strokeStyle = this.stroke; ctx.lineWidth = .72; ctx.stroke();
          if (item.shine) {
            ctx.globalAlpha = (item.shine - .7) * .34;
            ctx.strokeStyle = "#fff"; ctx.lineWidth = .45; ctx.stroke();
            ctx.globalAlpha = 1;
          }
        } else if (item.kind === "shadow") {
          ctx.fillStyle = `rgba(0,0,0,${item.alpha})`;
          ctx.beginPath(); ctx.ellipse(item.x, item.y + 1.4, item.rx, item.ry, 0, 0, 7); ctx.fill();
        } else {
          const gradient = ctx.createRadialGradient(item.x, item.y, 0,
            item.x, item.y, Math.max(1, item.radius));
          gradient.addColorStop(0, tint(item.color, 1.55, item.alpha));
          gradient.addColorStop(.35, tint(item.color, 1.1, item.alpha * .68));
          gradient.addColorStop(1, tint(item.color, 1, 0));
          ctx.fillStyle = gradient; ctx.beginPath();
          ctx.arc(item.x, item.y, item.radius, 0, 7); ctx.fill();
        }
      }
      ctx.restore();
    }
  }

  const mapPoint = (p, origin, scale) => [origin[0] + p[0] * scale,
    origin[1] + p[1] * scale, origin[2] + p[2] * scale];

  function human(scene, options, origin = [0, 0, 0], size = 1) {
    const type = options.type;
    const walk = options.moving ? Math.sin(options.time * 8.5 + options.seed) : 0;
    const action = options.action;
    const p = point => mapPoint(point, origin, size);
    const armor = tint(options.color, .78);
    const cloth = options.family === "religious" ? "#eee4cc" : options.color;
    const skin = options.skin || "#e8c49b";
    const leftFoot = p([-2.1 - walk * 1.7, -.3, .8]);
    const rightFoot = p([2.1 + walk * 1.7, .3, .8]);
    const leftKnee = p([-1.6, walk * .7, 4.5]);
    const rightKnee = p([1.6, -walk * .7, 4.5]);
    scene.tube(p([-1.7, 0, 8]), leftKnee, 1.05 * size, "#273039", 6);
    scene.tube(leftKnee, leftFoot, .9 * size, "#20272e", 6);
    scene.tube(p([1.7, 0, 8]), rightKnee, 1.05 * size, "#273039", 6);
    scene.tube(rightKnee, rightFoot, .9 * size, "#20272e", 6);
    scene.box(p([0, 0, 11.5]), [7.4 * size, 3.8 * size, 8 * size], cloth, [0, 0, -.04]);
    if (!options.civilian && options.family !== "religious")
      scene.box(p([0, -.15, 14.6]), [7.7 * size, 4 * size, 2.1 * size], armor);
    scene.ellipsoid(p([0, 0, 18]), [3.05 * size, 2.7 * size, 3.25 * size], skin, 7, 3);
    if (!options.civilian && options.family !== "religious")
      scene.ellipsoid(p([0, 0, 19.1]), [3.25 * size, 2.9 * size, 1.55 * size], armor, 7, 2);

    const shoulderL = p([-3.4, 0, 14]), shoulderR = p([3.4, 0, 14]);
    const leftHand = p([-5.2, -.4, 9.6]);
    let rightHand = p([5.1 + action * 2, -1.2, 10.4 + action * 3.5]);
    if (FIREARM.has(type) || BOW.has(type)) rightHand = p([5.4, -2.6 - action, 13.1]);
    scene.tube(shoulderL, leftHand, .9 * size, skin, 6);
    scene.tube(shoulderR, rightHand, .9 * size, skin, 6);

    if (MELEE.has(type) || type === "warrior_monk") {
      scene.tube(p([4.6, -1.2, 10.7]), p([10 + action * 3, -3,
        20 - action * 7]), .42 * size, "#edf3f5", 5);
      scene.box(p([-5.1, -.2, 11.2]), [4.8 * size, 1 * size, 6.2 * size], armor,
        [0, 0, -.1]);
    } else if (SPEAR.has(type)) {
      scene.tube(p([4.2, -.7, 5]), p([7 + action * 5, -3,
        23 - action * 5]), .34 * size, "#7c5d3b", 6);
      scene.wedge(p([7 + action * 5, -3, 23.8 - action * 5]),
        [1.5 * size, 3 * size, 3.5 * size], "#d8e1e6");
    } else if (FIREARM.has(type)) {
      const recoil = action * 2.2;
      scene.tube(p([-1.5, -1.3, 12]), p([11 - recoil, -4, 15]),
        .65 * size, "#4d3b2c", 7);
      scene.tube(p([3, -2.5, 14]), p([14 - recoil, -4.5, 15.3]),
        .42 * size, "#303940", 7);
      if (action > .58) scene.glow(p([15 - recoil, -4.7, 15.4]), 4.2 * size,
        "#ffcc6b", (action - .58) * 2);
    } else if (BOW.has(type)) {
      const pull = action * 2.6;
      const top = p([8, -2, 19]), mid = p([5 - pull, -3.5, 13.5]), bot = p([8, -2, 8]);
      scene.tube(top, p([10, -2, 16]), .28 * size, "#8c653d", 5);
      scene.tube(p([10, -2, 16]), bot, .28 * size, "#8c653d", 5);
      scene.tube(top, mid, .12 * size, "#eadfbe", 4);
      scene.tube(mid, bot, .12 * size, "#eadfbe", 4);
      scene.tube(mid, p([15 - pull, -4, 13.5]), .17 * size, "#d9e1df", 5);
    } else if (options.family === "religious") {
      scene.tube(p([5, 0, 5]), p([7 + action * 2, -1, 22 + action * 2]),
        .45 * size, type === "inquisitor" ? "#d95b3e" : "#c7a84b", 7);
      scene.glow(p([7 + action * 2, -1, 23 + action * 2]),
        (2.2 + action * 3.5) * size, type === "inquisitor" ? "#ff7648" : "#ffe58c",
        .35 + action * .45);
    } else if (type === "builder" || type === "military_engineer" || type === "archaeologist") {
      scene.tube(p([4, 0, 7]), p([9 + action * 2, -1, 19]), .38 * size,
        "#795634", 6);
      scene.box(p([9.5 + action * 2, -1, 19.5]), [5 * size, 2 * size, 2.8 * size],
        "#929da3", [0, 0, .25]);
    } else if (type === "rock_band") {
      scene.ellipsoid(p([6, -1, 11.5]), [3.8 * size, 1.4 * size, 5.2 * size],
        "#c85c92", 7, 3);
      scene.tube(p([4, -1, 13]), p([10, -2, 20]), .35 * size, "#d8c59e", 6);
    } else if (type === "medic") {
      scene.box(p([-4.2, .2, 11]), [3.6 * size, 2 * size, 6 * size], "#ece9dc");
      scene.box(p([-4.2, -1, 11]), [2.6 * size, .4 * size, .7 * size], "#d94b48");
      scene.box(p([-4.2, -1, 11]), [.7 * size, .4 * size, 2.6 * size], "#d94b48");
    } else if (type === "spy") {
      scene.polygon([p([-5, 0, 7]), p([0, 1, 17]), p([5, 0, 7])], "#202631", .7);
      scene.glow(p([1.2, -2.5, 18]), .8 * size, "#d9f4ff", .8);
    } else if (type === "naturalist") {
      scene.tube(p([4, 0, 7]), p([8, -1, 20]), .36 * size, "#355b38", 6);
      scene.ellipsoid(p([8, -1, 21]), [2 * size, 1.4 * size, 1 * size], "#c8dcc1", 6, 2);
    } else if (type === "trader" || type === "settler") {
      scene.box(p([-6, .8, 7]), [6 * size, 4 * size, 5 * size], "#a77e4d");
      scene.tube(p([-8, 1, 4]), p([-8, 1, 1]), 1.2 * size, "#41362a", 7);
      scene.tube(p([-4, 1, 4]), p([-4, 1, 1]), 1.2 * size, "#41362a", 7);
    }
  }

  function drawMounted(scene, o) {
    const stride = o.moving ? Math.sin(o.time * 9 + o.seed) : 0;
    scene.shadow(12, 4.2);
    scene.ellipsoid([0, 0, 8], [10.5, 4.2, 5.4], "#765333", 9, 4);
    scene.tube([7, 0, 9], [10, 0, 15], 2.2, "#765333", 7);
    scene.ellipsoid([10.5, 0, 16], [3.4, 2.8, 2.7], "#765333", 7, 3);
    for (const [x, phase] of [[-6, 1], [-2, -1], [3, -1], [7, 1]]) {
      const swing = stride * phase * 2.4;
      scene.tube([x, phase * 1.2, 6], [x + swing * .4, phase * 1.4, 2.7], .75,
        "#553a27", 6);
      scene.tube([x + swing * .4, phase * 1.4, 2.7], [x + swing, phase * 1.1, .4],
        .62, "#443024", 6);
    }
    scene.tube([-9, 0, 9], [-14, 1.4 + stride, 12], .5, "#3c2b22", 6);
    human(scene, {...o, civilian:false}, [-1, 0, 9], .7);
  }

  function drawArmor(scene, o) {
    const recoil = o.action * 3.2;
    scene.shadow(13, 4.4);
    scene.box([0, 0, 4.5], [23, 9, 6.5], "#273239");
    for (const y of [-5, 5]) for (const x of [-8, -3, 3, 8])
      scene.tube([x, y - .5, 2.4], [x, y + .5, 2.4], 2.4, "#566168", 7);
    scene.wedge([0, 0, 8.5], [18, 8, 6], tint(o.color, .76), 1);
    scene.ellipsoid([2, 0, 12], [6.4, 4.4, 3.2], o.color, 8, 3);
    if (o.type === "mobile_sam" || o.type === "anti_air_gun") {
      for (const y of [-2, 2]) scene.tube([0, y, 13], [11, y, 20], .75, "#c2cdd0", 7);
      scene.box([0, 0, 14], [8, 6, 2.5], tint(o.color, .9), [0, -.28, 0]);
    } else {
      scene.tube([3, 0, 13], [17 - recoil, 0, 16], 1.05, "#303b42", 8);
      if (o.action > .6) scene.glow([18 - recoil, 0, 16], 5, "#ffc76a", o.action);
    }
  }

  function drawRobot(scene, o) {
    const stride = o.moving ? Math.sin(o.time * 7 + o.seed) * 2 : 0;
    scene.shadow(10, 4.5, .34);
    for (const side of [-1, 1]) {
      scene.tube([side * 4, 0, 10], [side * (5 + stride), 0, 5.5], 1.6, "#34424b", 7);
      scene.tube([side * (5 + stride), 0, 5.5], [side * (5.5 + stride * 1.5), 0, .8],
        1.25, "#27333b", 7);
      scene.tube([side * 6, 0, 18], [side * (12 + o.action * 2), -1, 13 + o.action * 4],
        1.5, "#34424b", 7);
      scene.glow([side * (12 + o.action * 2), -1, 13 + o.action * 4], 2.8,
        "#77edff", .75);
    }
    scene.box([0, 0, 15], [13, 7, 12], o.color, [0, 0, -.03]);
    scene.box([0, -1, 22], [9, 6, 4.5], tint(o.color, .72));
    scene.glow([0, -4.2, 22.5], 4, "#7ff6ff", .9);
  }

  function drawGun(scene, o) {
    const recoil = o.action * 3.4;
    scene.shadow(12, 4);
    scene.box([-1, 0, 4], [15, 7, 3.5], tint(o.color, .72));
    for (const y of [-4, 4]) scene.tube([-5, y, 3.2], [-5, y + .5, 3.2], 3.2,
      "#303940", 8);
    if (o.type === "rocket_artillery") {
      scene.box([2, 0, 10], [12, 7, 4.5], "#424d52", [0, -.32, 0]);
      for (const y of [-2.2, 0, 2.2]) scene.tube([3, y, 10], [15 - recoil, y, 16],
        .72, "#a7b1b5", 7);
    } else {
      scene.tube([-1, 0, 8], [17 - recoil, 0, 15], o.type === "machine_gun" ? .65 : 1.05,
        "#343f45", 8);
      scene.box([-1, 0, 8], [7, 6, 4], o.color, [0, -.28, 0]);
    }
    human(scene, {...o, type:"infantry", action:o.action * .4}, [-7, 5, 0], .55);
    if (o.action > .62) scene.glow([18 - recoil, 0, 15], 5, "#ffc76a", o.action);
  }

  function drawSiege(scene, o) {
    const throwAngle = -.45 - o.action * 1.15;
    scene.shadow(12, 4);
    for (const y of [-4, 4]) scene.tube([-5, y, 3], [-5, y + .5, 3], 3, "#4b3626", 8);
    scene.box([0, 0, 5], [18, 7, 3], "#755435");
    if (o.type === "battering_ram") {
      scene.tube([-10 - o.action * 3, 0, 8], [10 + o.action * 3, 0, 8], 1.7,
        "#4d3524", 8);
      for (const x of [-7, 7]) scene.tube([x, -3, 4], [x, -3, 13], .65, "#725235", 6);
    } else if (o.type === "siege_tower") {
      scene.box([0, 0, 13], [11, 8, 20], "#725235");
      for (const z of [7, 13, 19]) scene.box([0, -4.2, z], [12, 1, 1], "#4b3626");
    } else {
      for (const y of [-3, 3]) {
        scene.tube([-5, y, 5], [0, y, 17], .65, "#6d4c30", 6);
        scene.tube([5, y, 5], [0, y, 17], .65, "#6d4c30", 6);
      }
      const tip = [Math.cos(throwAngle) * 17, 0, 8 - Math.sin(throwAngle) * 17];
      scene.tube([-4, 0, 7], tip, .7, "#553b28", 7);
      scene.ellipsoid(tip, [2.6, 2.4, 2.6], "#64666a", 7, 3);
    }
  }

  function drawNaval(scene, o, embarked = false) {
    scene.shadow(15, 4.2, .2);
    const modern = MODERN_SHIPS.has(o.type), submarine = SUBMARINES.has(o.type);
    const hull = embarked ? "#714d30" : (modern || submarine ? "#42515b" : "#68482f");
    scene.wedge([0, 0, 5], [25, 9, 7], hull, 1);
    if (submarine) {
      scene.ellipsoid([0, 0, 6.5], [12, 4.4, 4], hull, 10, 4);
      scene.box([-1, 0, 11], [5, 4, 4], "#35434b");
      scene.tube([0, 0, 12], [0, 0, 17], .38, "#809096", 6);
    } else if (modern) {
      const deck = o.type === "aircraft_carrier" ? [25, 8, 1.4] : [13, 7, 1.8];
      scene.box([0, 0, 9], deck, tint(o.color, .82));
      if (o.type === "aircraft_carrier") {
        scene.polygon([[-8, -2, 10], [5, -2, 10], [0, -2, 11.2]], "#d8e3e5", .45);
        scene.box([7, 2, 12], [5, 4, 6], "#657078");
      } else {
        scene.ellipsoid([4, 0, 12], [3.4, 3, 2.2], o.color, 7, 3);
        scene.tube([5, 0, 13], [15 - o.action * 3, 0, 15], .65, "#2e3940", 7);
      }
    } else {
      scene.tube([0, 0, 7], [0, 0, 24], .45, "#533a28", 7);
      scene.polygon([[0, 0, 23], [0, 0, 9], [10, 0, 14]], o.color, .5);
      if (embarked) human(scene, {...o, type:"settler", civilian:true}, [-4, 0, 7], .45);
    }
    const wake = Math.sin(o.time * 4 + o.seed) * 1.2;
    scene.glow([-12, 4 + wake, 2], 4.5, "#bfeef4", .22);
  }

  function drawAir(scene, o) {
    const jet = o.type !== "biplane";
    scene.shadow(13, 3.5, .13);
    scene.tube([-11, 0, 12], [13, 0, 12], 2.1, tint(o.color, .82), 9);
    scene.wedge([12, 0, 12], [5, 5, 4], tint(o.color, 1.15), 1);
    scene.polygon([[-5, -2, 12], [5, -15, 12], [8, -2, 12],
      [8, 2, 12], [5, 15, 12], [-5, 2, 12]], o.color, .8);
    scene.polygon([[-9, -1, 12], [-13, -7, 14], [-7, -1, 14],
      [-7, 1, 14], [-13, 7, 14], [-9, 1, 12]], tint(o.color, .75), .65);
    if (jet) {
      scene.glow([-12, 0, 12], 4 + o.action * 3, "#6de7ff", .5 + o.action * .35);
    } else {
      const spin = o.time * 20;
      scene.tube([14, Math.cos(spin) * 7, 12 + Math.sin(spin) * 7],
        [14, -Math.cos(spin) * 7, 12 - Math.sin(spin) * 7], .2, "#eee5cf", 5);
      scene.polygon([[-4, -14, 16], [5, -14, 16], [6, 14, 16], [-4, 14, 16]],
        tint(o.color, 1.08), .5);
    }
  }

  function drawRotor(scene, o) {
    const spin = o.time * 22;
    scene.shadow(12, 4, .14);
    scene.ellipsoid([2, 0, 11], [9, 5, 5.5], o.color, 9, 4);
    scene.tube([-3, 0, 12], [-17, 0, 15], 1.2, "#344148", 8);
    scene.box([-17, 0, 15], [2, 7, 6], tint(o.color, .72));
    scene.tube([0, 0, 17], [Math.cos(spin) * 18, Math.sin(spin) * 18, 17.3], .22,
      "#d8e5e5", 5);
    scene.tube([0, 0, 17], [-Math.cos(spin) * 18, -Math.sin(spin) * 18, 17.3], .22,
      "#d8e5e5", 5);
    scene.glow([7, -4.2, 11], 2.2, "#9fe5f1", .45);
  }

  function drawBalloon(scene, o) {
    const sway = Math.sin(o.time * 1.7 + o.seed) * .8;
    scene.shadow(7, 2.4, .12);
    scene.ellipsoid([sway, 0, 23], [8, 7, 11], tint(o.color, .74), 10, 5);
    scene.ellipsoid([sway - 2, -2, 25], [3, 2.3, 7], tint(o.color, 1.22), 7, 4);
    for (const x of [-3, 3]) scene.tube([x + sway, 0, 13], [x * .55, 0, 7], .18,
      "#7c6344", 5);
    scene.box([0, 0, 5.5], [6, 5, 5], "#795936");
  }

  function drawDrone(scene, o) {
    const spin = o.time * 28;
    scene.shadow(9, 3, .12);
    scene.box([0, 0, 12], [9, 6, 4], "#56666f");
    for (const sx of [-1, 1]) for (const sy of [-1, 1]) {
      const hub = [sx * 9, sy * 7, 13];
      scene.tube([sx * 3, sy * 2, 12], hub, .5, "#9aa8ad", 6);
      scene.tube([hub[0] + Math.cos(spin * sx) * 5, hub[1] + Math.sin(spin * sx) * 5, 13],
        [hub[0] - Math.cos(spin * sx) * 5, hub[1] - Math.sin(spin * sx) * 5, 13],
        .15, "#d5dfdf", 5);
    }
    scene.glow([2, -3.2, 11], 2.3, "#64ecff", .7);
  }

  function draw(options) {
    if (!options || !FAMILY_SET.has(options.family) || !options.ctx) return false;
    const o = {
      ...options,
      time: Number(options.time || 0) / 1000,
      action: clamp(Number(options.action || 0), 0, 1),
      seed: Number(options.seed || 0) * 1.713,
    };
    const scene = new Scene(o.ctx, {scale:o.scale || 1.04, facing:o.facing,
      bank:o.family === "air" ? Math.sin(o.time * 2.2 + o.seed) * .45 : 0});
    if (o.family === "mounted") drawMounted(scene, o);
    else if (o.family === "armor") drawArmor(scene, o);
    else if (o.family === "robot") drawRobot(scene, o);
    else if (o.family === "gun") drawGun(scene, o);
    else if (o.family === "siege") drawSiege(scene, o);
    else if (o.family === "naval") drawNaval(scene, o);
    else if (o.family === "embarked") drawNaval(scene, o, true);
    else if (o.family === "air") drawAir(scene, o);
    else if (o.family === "rotor") drawRotor(scene, o);
    else if (o.family === "balloon") drawBalloon(scene, o);
    else if (o.family === "drone") drawDrone(scene, o);
    else {
      scene.shadow(o.family === "civilian" ? 7 : 8.5, 3.2);
      human(scene, {...o, civilian:o.family === "civilian"});
    }
    scene.flush();
    return true;
  }

  global.Cinematic3D = Object.freeze({families:FAMILIES, supports:family => FAMILY_SET.has(family), draw});
})(globalThis);
