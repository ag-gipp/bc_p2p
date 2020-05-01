//! Type `PUT my-key my-value`
//! Type `GET my-key`
//! Close with Ctrl-c.

use async_std::{io, task};
use futures::prelude::*;
use libp2p::kad::record::store::MemoryStore;
//no persistent DB
use libp2p::kad::{record::Key, Kademlia, KademliaEvent, PutRecordOk, Quorum, Record};
// flex DTH algo, record = key != hash, callback event, ...
use libp2p::{
    NetworkBehaviour,
    PeerId, //hash
    Swarm, //?
    build_development_transport, //?
    identity, //PubKey
    mdns::{Mdns, MdnsEvent}, // initial peer discovery = only in LAN or VPN
    swarm::NetworkBehaviourEventProcess, // ?
};
use std::{error::Error, task::{Context, Poll}};

// We create a custom network behaviour that combines Kademlia and mDNS.
#[derive(NetworkBehaviour)] // behaviour = interface
struct MyBehaviour {
    kademlia: Kademlia<MemoryStore>,
    // non persistent key value store
    mdns: Mdns, //peer discovery - not usable in a open setting - hard coded peers needed
}

impl NetworkBehaviourEventProcess<MdnsEvent> for MyBehaviour {
    // implements the Peer-discovery interface method of NetworkBehaviour
// Called when `mdns` produces an event.
    fn inject_event(&mut self, event: MdnsEvent) {
        if let MdnsEvent::Discovered(list) = event {
            for (peer_id, multiaddr) in list {  //Multiaddr works with a variant of addresses (IPv4 for TCP in our case)
                self.kademlia.add_address(&peer_id, multiaddr);
            }
        }
    }
}

impl NetworkBehaviourEventProcess<KademliaEvent> for MyBehaviour {
    // implements the DHT interface method of NetworkBehavior
// Called when `kademlia` produces an event.
    fn inject_event(&mut self, message: KademliaEvent) {
        match message {
            KademliaEvent::GetRecordResult(Ok(result)) => {
                for Record { key, value, publisher, .. } in result.records {
                    println!(
                    "Got record {:?} {:?} from Publisher {:?}",
                    std::str::from_utf8(key.as_ref()),
                    std::str::from_utf8(&value),
                    publisher  //PRINT THE Publisher PEER
                );
                }
            }
            KademliaEvent::GetRecordResult(Err(err)) => {
                eprintln!("Failed to get record: {:?}", err);
            }
            KademliaEvent::PutRecordResult(Ok(PutRecordOk { key })) => {
                println!(
                    "Successfully put record {:?}",
                    std::str::from_utf8(key.as_ref())
                );
            }
            KademliaEvent::PutRecordResult(Err(err)) => {
                eprintln!("Failed to put record: {:?}", err);
            }
            _ => {}
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> { // return type "Result" for debug error handling
    env_logger::init(); //console logs

    // Create a random key for ourselves.
    let local_key = identity::Keypair::generate_ed25519();
    let local_peer_id = PeerId::from(local_key.public());

    // Set up a an encrypted DNS-enabled TCP Transport over the Mplex protocol.
    let transport = build_development_transport(local_key)?; // Transport layer ist variable, for our use case of small keys TCP is not ideal

    // Create a swarm to manage peers and events.
    let mut swarm = {  // swarm is like a channel - channel initialized with own peer_id
        // Create a Kademlia behaviour.
        let store = MemoryStore::new(local_peer_id.clone());
        let kademlia = Kademlia::new(local_peer_id.clone(), store); //keys and routing table
        let mdns = Mdns::new()?;
        let behaviour = MyBehaviour { kademlia, mdns };
        Swarm::new(transport, behaviour, local_peer_id.clone())
    };

    // Listen on all interfaces and whatever port the OS assigns.
    Swarm::listen_on(&mut swarm, "/ip4/0.0.0.0/tcp/0".parse()?)?; //listening for mdns results on addr


    //TODO: Dialog for Batch selection


    //TODO: Auto Upload hashes with own

    // Save cli input - blocking when busy
    helper_safe_cli(&mut swarm,  local_peer_id)
}

fn helper_safe_cli(swarm: &mut Swarm<MyBehaviour, PeerId>, local_peer_id: PeerId) -> Result<(), Box<dyn Error>> {
    // Read full lines from stdin
    let mut stdin = io::BufReader::new(io::stdin()).lines(); //cli input + buffer for increased performance

    let mut listening = false;
    task::block_on(future::poll_fn(move |cx: &mut Context| { //handle input as async task + context for timeout handling
        // blocking all new inputs until task is complete - poll fn checks for task completion
        loop {
            match stdin.try_poll_next_unpin(cx)? { //handle input, empty input, and pending input
                Poll::Ready(Some(line)) => handle_input_line(&mut swarm.kademlia, line, local_peer_id.clone()), //execute input command
                Poll::Ready(None) => panic!("Stdin closed"),
                Poll::Pending => break
            }
        }
        loop {
            match swarm.poll_next_unpin(cx) { //blocking until saving task has finished
                Poll::Ready(Some(event)) => println!("{:?}", event),
                Poll::Ready(None) => return Poll::Ready(Ok(())),
                Poll::Pending => {
                    if !listening {
                        if let Some(a) = Swarm::listeners(&swarm).next() {
                            println!("Listening on {:?}", a);
                            listening = true;
                        }
                    }
                    break;
                }
            }
        }
        Poll::Pending
    }))
}

// Handle commands
fn handle_input_line(kademlia: &mut Kademlia<MemoryStore>, line: String, local_peer_id: PeerId) {
    let mut args = line.split(" ");

    match args.next() {
        Some("GET") => {
            let key = {
                match args.next() {
                    Some(key) => Key::new(&key),
                    None => {
                        eprintln!("Expected key");
                        return;
                    }
                }
            };
            kademlia.get_record(&key, Quorum::One);
        }
        Some("PUT") => {
            let key = {
                match args.next() {
                    Some(key) => Key::new(&key),
                    None => {
                        eprintln!("Expected key");
                        return;
                    }
                }
            };
            let value = {
                match args.next() {
                    Some(value) => value.as_bytes().to_vec(),
                    None => {
                        eprintln!("Expected value");
                        return;
                    }
                }
            };
            let record = Record {
                key,
                value,
                publisher: Some(local_peer_id), // USEFUL FOR TRACEABILITY AND SPAM-PROTECTION TODO: issue - WHY doesn't it require a moveable type
                expires: None, //stays in memory for ever + periodic replication and republication
            };
            kademlia.put_record(record, Quorum::One); // Quorum = min replication factor specifies the minimum number of distinct nodes that must be successfully contacted in order for a query to succeed.
        }
        _ => {
            eprintln!("expected GET or PUT");
        }
    }
}
