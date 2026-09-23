// Package mobile connects a platform packet interface to MyProxy's policy and
// Xray's existing protocol implementations. It does not run a proxy listener.
package mobile

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"os"
	"strconv"
	"sync"
	"sync/atomic"
	"syscall"
	"time"

	xnet "github.com/xtls/xray-core/common/net"
	"github.com/xtls/xray-core/common/session"
	"github.com/xtls/xray-core/core"
	_ "github.com/xtls/xray-core/main/distro/all"
	"github.com/xtls/xray-core/transport/internet"
	"gvisor.dev/gvisor/pkg/buffer"
	"gvisor.dev/gvisor/pkg/tcpip"
	"gvisor.dev/gvisor/pkg/tcpip/adapters/gonet"
	"gvisor.dev/gvisor/pkg/tcpip/header"
	"gvisor.dev/gvisor/pkg/tcpip/link/channel"
	"gvisor.dev/gvisor/pkg/tcpip/network/ipv4"
	"gvisor.dev/gvisor/pkg/tcpip/network/ipv6"
	"gvisor.dev/gvisor/pkg/tcpip/stack"
	"gvisor.dev/gvisor/pkg/tcpip/transport/tcp"
	"gvisor.dev/gvisor/pkg/tcpip/transport/udp"
	"gvisor.dev/gvisor/pkg/waiter"
)

const (
	mtu = 1280
	maxFlows = 128
	maxRecent = 128
)

type Policy interface {
	Decide(requestJSON string) string
	Health(node string, delayMs int64, failed bool)
}

type Protector interface { Protect(fd int64) bool }
type PacketWriter interface { WritePacket(packet []byte) bool }

type node struct { Name string `json:"name"`; Tag string `json:"tag"` }
type render struct {
	Config string `json:"config"`
	Nodes []node `json:"nodes"`
	Revision uint64 `json:"revision"`
	BootstrapDNS string `json:"bootstrapDNS"`
}

type route struct {
	Action string `json:"action"`
	Tag *string `json:"tag"`
	Node *string `json:"node"`
	Rule string `json:"rule"`
	Chain []string `json:"chain"`
	Revision uint64 `json:"revision"`
}

type flowView struct {
	ID string `json:"id"`
	Host string `json:"host"`
	Port uint16 `json:"port"`
	Network string `json:"network"`
	Outbound string `json:"outbound"`
	Rule string `json:"rule"`
	Chain []string `json:"chain"`
	UploadBytes int64 `json:"uploadBytes"`
	DownloadBytes int64 `json:"downloadBytes"`
	StartedAt int64 `json:"startedAt"`
}

type flow struct {
	view flowView
	up atomic.Int64
	down atomic.Int64
	cancel context.CancelFunc
	local net.Conn
}

type Engine struct {
	ctx context.Context
	cancel context.CancelFunc
	instance *core.Instance
	stack *stack.Stack
	link *channel.Endpoint
	policy Policy
	protector Protector
	allowDirect bool
	nodes []node
	tags map[string]bool
	startedAt atomic.Int64
	closed atomic.Bool
	started atomic.Bool
	probeBusy atomic.Bool
	sequence atomic.Uint64
	up atomic.Int64
	down atomic.Int64
	mu sync.Mutex
	flows map[string]*flow
	recent []flowView
	tun *os.File
	names map[string]dnsName
	previousResolver *net.Resolver
}

var process struct {
	sync.Mutex
	engine *Engine
	registered bool
}

// NewEngine validates the complete outbound configuration before accepting any
// packets. Only one engine can own Xray's process-wide socket controller.
func NewEngine(renderJSON string, policy Policy, protector Protector, allowDirect bool) (*Engine, error) {
	if policy == nil || len(renderJSON) > 8*1024*1024 { return nil, errors.New("代理配置无效") }
	var cfg render
	if err := json.Unmarshal([]byte(renderJSON), &cfg); err != nil || len(cfg.Nodes) == 0 || len(cfg.Nodes) > 2048 {
		return nil, errors.New("没有可用节点或代理配置无效")
	}
	if allowDirect && protector == nil { return nil, errors.New("Android 网络保护未准备好") }
	ctx, cancel := context.WithCancel(context.Background())
	e := &Engine{ctx:ctx, cancel:cancel, policy:policy, protector:protector, allowDirect:allowDirect,
		nodes:cfg.Nodes, tags:make(map[string]bool), flows:make(map[string]*flow), names:make(map[string]dnsName)}
	for _, n := range cfg.Nodes { if n.Tag == "" || e.tags[n.Tag] { cancel(); return nil, errors.New("节点标识无效") }; e.tags[n.Tag] = true }
	process.Lock()
	if process.engine != nil { process.Unlock(); cancel(); return nil, errors.New("上一个代理尚未停止") }
	if !process.registered {
		if err := internet.RegisterDialerController(protectSocket); err != nil { process.Unlock(); cancel(); return nil, errors.New("无法准备代理网络") }
		process.registered = true
	}
	process.engine = e
	process.Unlock()
	bootstrap := cfg.BootstrapDNS
	if bootstrap == "" { bootstrap = "1.1.1.1:53" }
	if host, port, err := net.SplitHostPort(bootstrap); err != nil || net.ParseIP(host) == nil || port != "53" {
		e.Close(); return nil, errors.New("启动 DNS 地址无效")
	}
	e.previousResolver = net.DefaultResolver
	net.DefaultResolver = &net.Resolver{PreferGo:true, Dial:func(ctx context.Context, network, _ string) (net.Conn,error) {
		d := net.Dialer{Timeout:5*time.Second, Control:protectSocket}
		return d.DialContext(ctx, network, bootstrap)
	}}
	instance, err := core.StartInstance("json", []byte(cfg.Config))
	if err != nil { e.Close(); return nil, errors.New("节点配置未通过 Xray 验证") }
	e.instance = instance
	e.link = channel.New(256, mtu, "")
	e.stack = stack.New(stack.Options{
		NetworkProtocols:[]stack.NetworkProtocolFactory{ipv4.NewProtocol, ipv6.NewProtocol},
		TransportProtocols:[]stack.TransportProtocolFactory{tcp.NewProtocol, udp.NewProtocol},
	})
	if err := e.stack.CreateNIC(1,e.link); err != nil { e.Close(); return nil, fmt.Errorf("无法创建隧道接口: %s",err) }
	if err := e.stack.SetPromiscuousMode(1,true); err != nil { e.Close(); return nil, errors.New("无法接入隧道流量") }
	if err := e.stack.SetSpoofing(1,true); err != nil { e.Close(); return nil, errors.New("无法准备隧道地址") }
	e.stack.SetRouteTable([]tcpip.Route{{Destination:header.IPv4EmptySubnet,NIC:1},{Destination:header.IPv6EmptySubnet,NIC:1}})
	e.stack.SetTransportProtocolHandler(tcp.ProtocolNumber,tcp.NewForwarder(e.stack,32*1024,maxFlows,e.acceptTCP).HandlePacket)
	e.stack.SetTransportProtocolHandler(udp.ProtocolNumber,func(id stack.TransportEndpointID,packet *stack.PacketBuffer)bool{
		return e.acceptUDP(udp.NewForwarderRequest(e.stack,id,packet))
	})
	return e,nil
}

func protectSocket(_, _ string, raw syscall.RawConn) error {
	process.Lock(); e := process.engine; process.Unlock()
	if e == nil || e.closed.Load() { return errors.New("代理已停止") }
	if e.protector == nil { return nil }
	var rejected bool
	if err := raw.Control(func(fd uintptr) { rejected = !e.protector.Protect(int64(fd)) }); err != nil { return err }
	if rejected { return errors.New("无法保护代理连接") }
	return nil
}

func (e *Engine) Start(writer PacketWriter) error {
	if writer == nil || e.closed.Load() || !e.started.CompareAndSwap(false,true) { return errors.New("隧道状态无效") }
	e.startedAt.Store(time.Now().UnixMilli())
	go func() {
		for {
			packet := e.link.ReadContext(e.ctx)
			if packet == nil { return }
			view := packet.ToView()
			data := append([]byte(nil),view.AsSlice()...)
			view.Release(); packet.DecRef()
			if !writer.WritePacket(data) { e.Close(); return }
		}
	}()
	go func() {
		e.Probe()
		tick := time.NewTicker(5*time.Minute); defer tick.Stop()
		for { select { case <-e.ctx.Done(): return; case <-tick.C: e.Probe() } }
	}()
	return nil
}

type fileWriter struct { file *os.File }
func (w *fileWriter) WritePacket(packet []byte) bool { n,err := w.file.Write(packet); return err == nil && n == len(packet) }

func (e *Engine) StartTun(fd int64) error {
	if fd < 0 || fd > 1<<30 { return errors.New("隧道描述符无效") }
	duplicate, err := syscall.Dup(int(fd)); if err != nil { return errors.New("无法打开隧道") }
	if err = syscall.SetNonblock(duplicate,true); err != nil { syscall.Close(duplicate); return err }
	file := os.NewFile(uintptr(duplicate),"myproxy-tun")
	e.mu.Lock(); e.tun = file; e.mu.Unlock()
	if err = e.Start(&fileWriter{file}); err != nil { file.Close(); return err }
	go func() {
		packet := make([]byte,65535)
		for { n,err := file.Read(packet); if err != nil { e.Close(); return }; if err := e.WritePacket(packet[:n]); err != nil && e.closed.Load() { return } }
	}()
	return nil
}

func (e *Engine) WritePacket(packet []byte) error {
	if e.closed.Load() || !e.started.Load() { return errors.New("隧道未连接") }
	if len(packet) < 20 || len(packet) > 65535 { return errors.New("无效的数据包") }
	var protocol tcpip.NetworkProtocolNumber
	switch packet[0] >> 4 { case 4: protocol = ipv4.ProtocolNumber; case 6: if len(packet)<40{return errors.New("无效的数据包")}; protocol = ipv6.ProtocolNumber; default:return errors.New("未知网络协议") }
	pkt := stack.NewPacketBuffer(stack.PacketBufferOptions{Payload:buffer.MakeWithData(append([]byte(nil),packet...))})
	e.link.InjectInbound(protocol,pkt); pkt.DecRef()
	return nil
}

func (e *Engine) decide(id stack.TransportEndpointID, network string) (route,string,error) {
	host := id.LocalAddress.String()
	request := map[string]any{"op":"route","host":host,"port":id.LocalPort,"network":network}
	if name := e.hostname(host); name != "" { request["hostname"] = name }
	data,_ := json.Marshal(request)
	var reply struct { OK bool `json:"ok"`; Data route `json:"data"` }
	if len(data)>4096 { return route{},host,errors.New("连接信息过长") }
	response := e.policy.Decide(string(data))
	if len(response)>64*1024 || json.Unmarshal([]byte(response),&reply)!=nil || !reply.OK { return route{},host,errors.New("路由决策失败") }
	r := reply.Data
	if r.Action=="direct" && e.allowDirect { return r,host,nil }
	if r.Action!="proxy" || r.Tag==nil || !e.tags[*r.Tag] { return r,host,errors.New("没有可用出口") }
	return r,host,nil
}

func (e *Engine) reserve(id stack.TransportEndpointID, network string, r route, host string) (*flow,context.Context,error) {
	e.mu.Lock(); defer e.mu.Unlock()
	if e.closed.Load() || len(e.flows)>=maxFlows { return nil,nil,errors.New("连接数量达到上限") }
	ctx,cancel := context.WithCancel(e.ctx)
	outbound := "直连"; if r.Node!=nil { outbound = *r.Node }
	chain := r.Chain; if chain==nil { chain=[]string{} }
	f := &flow{cancel:cancel, view:flowView{ID:strconv.FormatUint(e.sequence.Add(1),10),Host:host,Port:id.LocalPort,Network:network,Outbound:outbound,Rule:r.Rule,Chain:chain,StartedAt:time.Now().UnixMilli()}}
	e.flows[f.view.ID]=f
	return f,ctx,nil
}

func (e *Engine) finish(f *flow) {
	f.cancel()
	e.mu.Lock();local:=f.local;e.mu.Unlock()
	if local!=nil { local.Close() }
	e.mu.Lock(); defer e.mu.Unlock()
	if _,ok:=e.flows[f.view.ID]; !ok{return}
	delete(e.flows,f.view.ID)
	v:=f.view; v.UploadBytes=f.up.Load(); v.DownloadBytes=f.down.Load()
	e.recent=append(e.recent,v)
	if len(e.recent)>maxRecent { e.recent=append([]flowView(nil),e.recent[len(e.recent)-maxRecent:]...) }
}

func (e *Engine) acceptTCP(request *tcp.ForwarderRequest) {
	id:=request.ID()
	r,host,err:=e.decide(id,"tcp"); if err!=nil {request.Complete(true);return}
	f,ctx,err:=e.reserve(id,"tcp",r,host); if err!=nil {request.Complete(true);return}
	go func() {
		defer e.finish(f)
		upstream,err:=e.dial(ctx,r,host,id.LocalPort,"tcp")
		if err!=nil {request.Complete(true);e.failed(r);return}; defer upstream.Close()
		var queue waiter.Queue
		endpoint,endpointErr:=request.CreateEndpoint(&queue)
		if endpointErr!=nil {request.Complete(true);return}
		request.Complete(false)
		local:=gonet.NewTCPConn(&queue,endpoint)
		e.mu.Lock(); f.local=local; e.mu.Unlock()
		stop:=context.AfterFunc(ctx,func(){local.Close();upstream.Close()}); defer stop()
		done:=make(chan struct{})
		go func(){copyTraffic(upstream,local,&f.up,&e.up);if c,ok:=upstream.(interface{CloseWrite() error});ok{c.CloseWrite()}else{upstream.Close()};close(done)}()
		copyTraffic(local,upstream,&f.down,&e.down)
		local.Close();upstream.Close();<-done
	}()
}

func copyTraffic(dst io.Writer,src io.Reader,flowBytes,total *atomic.Int64) {
	buf:=make([]byte,16*1024)
	for {n,readErr:=src.Read(buf);if n>0 {written,err:=dst.Write(buf[:n]);flowBytes.Add(int64(written));total.Add(int64(written));if err!=nil||written!=n{return}};if readErr!=nil{return}}
}

func (e *Engine) dial(ctx context.Context,r route,host string,port uint16,network string)(net.Conn,error){
	if r.Action=="direct" {
		if !e.allowDirect{return nil,errors.New("当前平台不支持动态直连")}
		d:=net.Dialer{Timeout:10*time.Second,Control:protectSocket}
		return d.DialContext(ctx,network,net.JoinHostPort(host,strconv.Itoa(int(port))))
	}
	if r.Tag==nil || !e.tags[*r.Tag] {return nil,errors.New("出口不可用")}
	ctx=session.SetForcedOutboundTagToContext(ctx,*r.Tag)
	destination:=xnet.TCPDestination(xnet.ParseAddress(host),xnet.Port(port))
	if network=="udp" {destination=xnet.UDPDestination(xnet.ParseAddress(host),xnet.Port(port))}
	return core.Dial(ctx,e.instance,destination)
}

func (e *Engine) failed(r route){if r.Node!=nil {e.policy.Health(*r.Node,-1,true)}}

func (e *Engine) CloseConnections(){
	e.mu.Lock(); flows:=make([]*flow,0,len(e.flows));for _,f:=range e.flows{flows=append(flows,f)};e.mu.Unlock()
	for _,f:=range flows {f.cancel();e.mu.Lock();local:=f.local;e.mu.Unlock();if local!=nil{local.Close()}}
}

func (e *Engine) Close(){
	if !e.closed.CompareAndSwap(false,true){return}
	e.cancel();e.CloseConnections()
	e.mu.Lock();file:=e.tun;e.tun=nil;e.mu.Unlock();if file!=nil{file.Close()}
	if e.stack!=nil{e.stack.Close()};if e.link!=nil{e.link.Close()}
	if e.instance!=nil{e.instance.Close()}
	process.Lock();if process.engine==e{process.engine=nil;if e.previousResolver!=nil{net.DefaultResolver=e.previousResolver}};process.Unlock()
}

func (e *Engine) Snapshot()string{
	e.mu.Lock();rows:=append([]flowView{},e.recent...);for _,f:=range e.flows{v:=f.view;v.UploadBytes=f.up.Load();v.DownloadBytes=f.down.Load();rows=append(rows,v)};e.mu.Unlock()
	phase:="connecting";if e.closed.Load(){phase="disconnected"}else if e.started.Load(){phase="connected"}
	var since any;if value:=e.startedAt.Load();value>0{since=value}
	data,_:=json.Marshal(map[string]any{"phase":phase,"message":nil,"connectedAt":since,"uploadBytes":e.up.Load(),"downloadBytes":e.down.Load(),"connections":rows})
	return string(data)
}
