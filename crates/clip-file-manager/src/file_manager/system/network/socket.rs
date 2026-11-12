//! Only RTM_GETLINK/RTM_GETADDR requests, without subscriptions or packet traffic.
use super::codec;
use std::{
    io,
    os::fd::OwnedFd,
    time::{Duration, Instant},
};
pub(super) struct Query {
    fd: OwnedFd,
    started: Instant,
    sequence: u32,
    bytes: usize,
    packets: usize,
}
impl Query {
    pub fn new() -> io::Result<Self> {
        let fd = rustix::net::socket_with(
            rustix::net::AddressFamily::NETLINK,
            rustix::net::SocketType::RAW,
            rustix::net::SocketFlags::CLOEXEC | rustix::net::SocketFlags::NONBLOCK,
            None,
        )?;
        rustix::net::bind(&fd, &rustix::net::netlink::SocketAddrNetlink::new(0, 0))?;
        Ok(Self {
            fd,
            started: Instant::now(),
            sequence: 0,
            bytes: 0,
            packets: 0,
        })
    }
    pub fn link(&mut self, index: u32) -> io::Result<codec::Link> {
        let sequence = self.send(index, false)?;
        let packets = self.receive(sequence, false)?;
        let mut links = packets
            .iter()
            .filter(|&(kind, _payload)| *kind == 16)
            .map(|(_kind, payload)| codec::link(payload))
            .collect::<io::Result<Vec<_>>>()?;
        if links.len() != 1 || links[0].index != index {
            return Err(codec::malformed());
        }
        Ok(links.remove(0))
    }
    pub fn addresses(&mut self, index: u32) -> io::Result<Vec<codec::Address>> {
        let sequence = self.send(index, true)?;
        let packets = self.receive(sequence, true)?;
        let mut addresses = Vec::new();
        for (kind, payload) in packets {
            if kind == 20
                && let Some(address) = codec::address(&payload)?
                && address.index == index
            {
                addresses.push(address);
            }
        }
        Ok(addresses)
    }
    fn send(&mut self, index: u32, addresses: bool) -> io::Result<u32> {
        self.sequence += 1;
        let request = request(index, addresses, self.sequence)?;
        let sent = rustix::net::sendto(
            &self.fd,
            &request,
            rustix::net::SendFlags::DONTWAIT,
            &rustix::net::netlink::SocketAddrNetlink::new(0, 0),
        )?;
        if sent as usize != request.len() {
            return Err(io::Error::from(io::ErrorKind::WriteZero));
        }
        Ok(self.sequence)
    }
    fn packet(&mut self) -> io::Result<Vec<u8>> {
        loop {
            let remaining = Duration::from_millis(500)
                .checked_sub(self.started.elapsed())
                .ok_or_else(|| io::Error::from(io::ErrorKind::TimedOut))?;
            if self.packets >= 128 || self.bytes >= 2 * 1024 * 1024 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Netlink response budget exhausted",
                ));
            }
            let mut fds = [rustix::event::PollFd::new(
                &self.fd,
                rustix::event::PollFlags::IN,
            )];
            let timeout = rustix::event::Timespec {
                tv_sec: 0,
                tv_nsec: remaining.as_millis().max(1) as i64 * 1_000_000,
            };
            let ready = match rustix::event::poll(&mut fds, Some(&timeout)) {
                Ok(ready) => ready,
                Err(rustix::io::Errno::INTR) => continue,
                Err(error) => return Err(error.into()),
            };
            if ready == 0 {
                return Err(io::Error::from(io::ErrorKind::TimedOut));
            }
            match self.datagram() {
                Ok(packet) => return Ok(packet),
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
                    ) =>
                {
                    continue;
                }
                Err(error) => return Err(error),
            }
        }
    }
    fn datagram(&mut self) -> io::Result<Vec<u8>> {
        let mut buffer = vec![0u8; 64 * 1024];
        let (_, count, sender) = rustix::net::recvfrom(
            &self.fd,
            buffer.as_mut_slice(),
            rustix::net::RecvFlags::DONTWAIT | rustix::net::RecvFlags::TRUNC,
        )?;
        if count == 0 {
            return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
        }
        let sender = sender
            .and_then(|address| rustix::net::netlink::SocketAddrNetlink::try_from(address).ok())
            .ok_or_else(codec::malformed)?;
        if sender.pid() != 0 || sender.groups() != 0 {
            return Err(codec::malformed());
        }
        self.packets += 1;
        self.bytes += count;
        if count > buffer.len() || self.bytes > 2 * 1024 * 1024 {
            return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
        }
        buffer.truncate(count);
        Ok(buffer)
    }
    fn receive(&mut self, sequence: u32, dump: bool) -> io::Result<Vec<(u16, Vec<u8>)>> {
        let mut records = Vec::new();
        loop {
            let packet = self.packet()?;
            for message in codec::messages(&packet)? {
                match response(&message, sequence, dump)? {
                    Reply::Skip => {}
                    Reply::Done => return Ok(records),
                    Reply::Data => {
                        if records.len() >= 1024 {
                            return Err(io::Error::new(
                                io::ErrorKind::UnexpectedEof,
                                "Netlink record limit reached",
                            ));
                        }
                        records.push((message.kind, message.payload.to_vec()));
                    }
                }
            }
        }
    }
}
#[derive(Debug, PartialEq)]
enum Reply {
    Skip,
    Done,
    Data,
}
fn response(message: &codec::Message<'_>, sequence: u32, dump: bool) -> io::Result<Reply> {
    if message.sequence != sequence {
        return Ok(Reply::Skip);
    }
    if message.flags & 0x10 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "Netlink dump interrupted; inventory incomplete",
        ));
    }
    match message.kind {
        2 => {
            check_error(codec::integer(message.payload, 0)? as i32)?;
            Ok(if dump { Reply::Skip } else { Reply::Done })
        }
        3 => {
            if message.payload.len() >= 4 {
                check_error(codec::integer(message.payload, 0)? as i32)?;
            }
            Ok(Reply::Done)
        }
        4 => Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "Netlink receive overrun",
        )),
        16 | 20 => Ok(Reply::Data),
        _ => Ok(Reply::Skip),
    }
}
fn request(index: u32, addresses: bool, sequence: u32) -> io::Result<Vec<u8>> {
    if index == 0 || index > i32::MAX as u32 {
        return Err(codec::malformed());
    }
    let length = if addresses { 24usize } else { 32usize };
    let mut request = vec![0; length];
    request[..4].copy_from_slice(&(length as u32).to_ne_bytes());
    request[4..6].copy_from_slice(&(if addresses { 22u16 } else { 18u16 }).to_ne_bytes());
    request[6..8].copy_from_slice(&(if addresses { 0x301u16 } else { 5u16 }).to_ne_bytes());
    request[8..12].copy_from_slice(&sequence.to_ne_bytes());
    request[20..24].copy_from_slice(&index.to_ne_bytes());
    Ok(request)
}
fn check_error(code: i32) -> io::Result<()> {
    if code == 0 {
        Ok(())
    } else if code < 0 {
        Err(io::Error::from_raw_os_error(
            code.checked_neg().ok_or_else(codec::malformed)?,
        ))
    } else {
        Err(codec::malformed())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn requests_can_only_query_links_or_dump_addresses_without_subscription_or_mutation() {
        let link = request(7, false, 11).unwrap();
        let addresses = request(7, true, 12).unwrap();
        assert_eq!(u16::from_ne_bytes(link[4..6].try_into().unwrap()), 18);
        assert_eq!(u16::from_ne_bytes(addresses[4..6].try_into().unwrap()), 22);
        assert_eq!(u16::from_ne_bytes(link[6..8].try_into().unwrap()), 5);
        assert_eq!(
            u16::from_ne_bytes(addresses[6..8].try_into().unwrap()),
            0x301
        );
        assert_eq!(codec::integer(&link, 20).unwrap(), 7);
        assert_eq!(codec::integer(&link, 24).unwrap(), 0);
        assert_eq!(codec::integer(&link, 28).unwrap(), 0);
        assert!(request(0, false, 1).is_err());
        assert!(request(u32::MAX, false, 1).is_err());
    }
    #[test]
    fn interrupted_dumps_and_wrong_sequences_do_not_report_complete_inventory() {
        let done = codec::Message {
            kind: 3,
            flags: 0x10,
            sequence: 7,
            payload: &[],
        };
        assert_eq!(
            response(&done, 7, true).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
        assert_eq!(response(&done, 8, true).unwrap(), Reply::Skip);
        let code = 0i32.to_ne_bytes();
        let ack = codec::Message {
            kind: 2,
            flags: 0,
            sequence: 7,
            payload: &code,
        };
        assert_eq!(response(&ack, 7, true).unwrap(), Reply::Skip);
        assert_eq!(response(&ack, 7, false).unwrap(), Reply::Done);
    }
    #[test]
    fn kernel_errors_do_not_become_successful_empty_inventories() {
        assert!(check_error(0).is_ok());
        assert_eq!(
            check_error(-libc::EACCES).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert!(check_error(1).is_err());
        assert!(check_error(i32::MIN).is_err());
    }
}
