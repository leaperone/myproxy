package mobile

import (
	"net"
	"strings"
	"time"

	"golang.org/x/net/dns/dnsmessage"
)

type dnsName struct { name string; expires time.Time; ambiguous bool }

func (e *Engine) hostname(ip string)string{
	e.mu.Lock();defer e.mu.Unlock()
	entry,ok:=e.names[ip];if !ok{return ""}
	if time.Now().After(entry.expires){delete(e.names,ip);return ""}
	if entry.ambiguous{return ""};return entry.name
}

func (e *Engine) observeDNS(query,response []byte){
	var q,r dnsmessage.Message
	if q.Unpack(query)!=nil||r.Unpack(response)!=nil||len(q.Questions)!=1||len(r.Questions)!=1||!r.Header.Response||r.Header.RCode!=dnsmessage.RCodeSuccess{return}
	if q.Questions[0]!=r.Questions[0]{return}
	name:=strings.TrimSuffix(strings.ToLower(q.Questions[0].Name.String()),".")
	if name==""||len(name)>253{return}
	now:=time.Now()
	e.mu.Lock();defer e.mu.Unlock()
	for _,answer:=range r.Answers{
		var ip string
		switch body:=answer.Body.(type){case *dnsmessage.AResource:ip=net.IP(body.A[:]).String();case *dnsmessage.AAAAResource:ip=net.IP(body.AAAA[:]).String();default:continue}
		ttl:=answer.Header.TTL;if ttl==0{continue};if ttl>300{ttl=300}
		prior,exists:=e.names[ip]
		ambiguous:=exists&&prior.expires.After(now)&&(prior.ambiguous||prior.name!=name)
		e.names[ip]=dnsName{name:name,expires:now.Add(time.Duration(ttl)*time.Second),ambiguous:ambiguous}
	}
	if len(e.names)>2048 {for ip,entry:=range e.names{if now.After(entry.expires){delete(e.names,ip)}};for ip:=range e.names{if len(e.names)<=2048{break};delete(e.names,ip)}}
}
