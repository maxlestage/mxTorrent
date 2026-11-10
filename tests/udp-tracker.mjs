// Tracker UDP minimal (BEP 15), IPv4 : de quoi relier Transmission et la passerelle.
import dgram from "node:dgram";
const swarms = new Map(); // info_hash hex → Map("ip:port" → [ip, port])
const sock = dgram.createSocket("udp4");
sock.on("message", (msg, rinfo) => {
  const action = msg.readUInt32BE(8), tid = msg.readUInt32BE(12);
  if (action === 0 && msg.length >= 16) {
    const r = Buffer.alloc(16); r.writeUInt32BE(0, 0); r.writeUInt32BE(tid, 4); r.writeBigUInt64BE(0x1234n, 8);
    return sock.send(r, rinfo.port, rinfo.address);
  }
  if (action === 1 && msg.length >= 98) {
    const ih = msg.subarray(16, 36).toString("hex"), port = msg.readUInt16BE(96), left = msg.readBigUInt64BE(64);
    const sw = swarms.get(ih) || new Map(); swarms.set(ih, sw);
    const others = [...sw.entries()].filter(([k]) => k !== `${rinfo.address}:${port}`).map(([, v]) => v);
    if (left === 0n) sw.set(`${rinfo.address}:${port}`, [rinfo.address, port]); // seuls les partageurs sont gardés
    console.log(`annonce ${ih.slice(0, 8)} de ${rinfo.address}:${port} (reste ${left}) → ${others.length} pairs`);
    const r = Buffer.alloc(20 + 6 * others.length);
    r.writeUInt32BE(1, 0); r.writeUInt32BE(tid, 4); r.writeUInt32BE(60, 8); r.writeUInt32BE(0, 12); r.writeUInt32BE(sw.size, 16);
    others.forEach(([ip, p], i) => { ip.split(".").forEach((o, j) => r.writeUInt8(+o, 20 + 6 * i + j)); r.writeUInt16BE(p, 24 + 6 * i); });
    return sock.send(r, rinfo.port, rinfo.address);
  }
});
sock.bind(8969, () => console.log("tracker UDP sur :8969"));
