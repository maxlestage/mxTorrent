// Tracker HTTP classique (bittorrent-tracker), IPv4, pour le test avec Transmission.
import { Server } from "bittorrent-tracker";
const server = new Server({ udp: false, http: true, ws: false, stats: false });
server.on("error", (e) => console.error("erreur", e.message));
server.on("start", (addr) => console.log("annonce HTTP de", addr));
server.listen(8970, "127.0.0.1", () => console.log("tracker HTTP sur 127.0.0.1:8970"));
