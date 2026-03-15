let rtc_connections = {};
let socket;

async function init() {
    // Open websocket stream
    socket = start_websocket();

    // Get list of cameras
    let cameras = await (await fetch('/api/cameras')).json();
    console.dir(cameras);

    // Find the sub stream for each camera (largest num)
    for (const camera of cameras) {
        if (camera.streams.length == 0) continue;
        let stream = camera.streams.reduce((max, stream) => max.stream_type > stream.stream_type ? max : stream);
        console.log('stream : ' + stream.stream_type);
        let element = await initialise_stream_rtc(camera.camera_id, stream.stream_id, stream.stream_type);
        console.dir(element);
        document.body.appendChild(element);
    }
}

// Opens the websocket and returns its reference
function start_websocket() {
    const socket = new WebSocket("ws://"+ window.location.hostname + ":8089");

    // Connection opened
    socket.addEventListener("open", (event) => {
        console.log("Socket open");
    });

    // Listen for messages
    socket.addEventListener("message", async (event) => {
        console.log("Message from server ", event.data);
        // Interpret message
        let msg = event.data;
        let segs = msg.split(":");

        switch(segs[0]) {
            case "RTC":
                // RTC req.
                // Pretty much only case is OPEN
                if (segs.length != 4 || segs[1] != "OPEN") return;

                // Find camera reference
                let stream = segs[2];
                let answer = segs[3];

                let pc = rtc_connections[stream];
                if (!pc) {
                    console.error("Could not find peer connection");
                    return;
                }

                const desc = new RTCSessionDescription(JSON.parse(atob(answer.trim())));
                await pc.setRemoteDescription(desc);
                console.log("Description set")
                break;
        }
    });

    return socket;
}

function request_rtc_stream(camera_id, stream_type, offer) {
    // TODO: Something better than a recursion to oflow
    if (socket.readyState != socket.OPEN) {
        // 500ms retry
        setTimeout(() => {request_rtc_stream(camera_id, stream_type, offer)}, 500);
        return;
    }
    socket.send(`RTC:OPEN:${camera_id}.${stream_type}:${offer}`);
}


// Creates the UI element for a video preview
async function initialise_stream_rtc(camera_id, stream_id, stream_type) {
    // First create the video element
    let vid = document.createElement('video');
    vid.id = `stream${stream_id}`;
    vid.autoplay = true;
    vid.controls = false;
    vid.playsInline = true;
    vid.muted = true;

    // Then make the RTC Connection
    let pc = new RTCPeerConnection({
        iceServers: [
            //{ urls: 'stun:stun.l.google.com:19302' }
        ]
    });

    // 1. Handle Incoming Video Track
    pc.ontrack = function (event) {
        console.log("Track received:", event.track.kind);
        vid.srcObject = event.streams[0];
    };

    // 2. Handle ICE Candidates
    pc.onicecandidate = e => {
        if (e.candidate === null) {
            // Send offer to server
            console.log("MAKE REQ");
            rtc_connections[`${camera_id}.${stream_type}`] = pc;
            request_rtc_stream(camera_id, stream_type, btoa(JSON.stringify(pc.localDescription)));
        }
    };


    pc.addTransceiver('video', { direction: 'recvonly' });

    const offer = await pc.createOffer();
    await pc.setLocalDescription(offer);


    // Once the websocket request completes, the peer conncetion will be updated.
    return vid;
}


window.onload = () => {init();}