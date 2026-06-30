import React from "react";
import { Link, useParams } from "wouter";
import { useGetTransaction } from "@workspace/api-client-react";
import { formatNumber, timeAgo, formatAddress } from "@/lib/utils";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { Badge } from "@/components/ui/badge";
import { ListOrdered, ArrowLeft, Code } from "lucide-react";

export default function TxDetail() {
  const { hash } = useParams<{ hash: string }>();
  const { data: tx, isLoading } = useGetTransaction(hash as string, { 
    query: { enabled: !!hash } 
  });

  if (isLoading) {
    return (
      <div className="space-y-6">
        <Skeleton className="h-8 w-64" />
        <Skeleton className="h-64 w-full" />
      </div>
    );
  }

  if (!tx) {
    return <div>Transaction not found</div>;
  }

  return (
    <div className="space-y-6">
      <div className="flex items-center gap-4 mb-4">
        <Link href="/txs" className="p-2 hover:bg-accent rounded-full transition-colors">
          <ArrowLeft className="w-5 h-5" />
        </Link>
        <div>
          <h2 className="text-2xl font-bold tracking-tight flex items-center gap-2">
            <ListOrdered className="w-6 h-6 text-primary" />
            Transaction Details
          </h2>
          <p className="text-muted-foreground font-mono text-xs">{tx.hash}</p>
        </div>
      </div>

      <div className="grid grid-cols-1 gap-6">
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm uppercase tracking-wider text-muted-foreground">Overview</CardTitle>
          </CardHeader>
          <CardContent className="space-y-4">
            <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
              <div className="space-y-4">
                <div className="flex justify-between py-2 border-b border-border/50">
                  <span className="text-muted-foreground text-sm">Status</span>
                  <Badge variant={tx.status === 'success' ? 'default' : tx.status === 'failed' ? 'destructive' : 'secondary'} className="text-[10px]">
                    {tx.status}
                  </Badge>
                </div>
                <div className="flex justify-between py-2 border-b border-border/50">
                  <span className="text-muted-foreground text-sm">Block</span>
                  <Link href={`/blocks/${tx.blockNumber}`} className="text-sm font-mono text-primary hover:underline">
                    {tx.blockNumber}
                  </Link>
                </div>
                <div className="flex justify-between py-2 border-b border-border/50">
                  <span className="text-muted-foreground text-sm">Timestamp</span>
                  <span className="text-sm font-medium">{new Date(tx.timestamp).toLocaleString()} ({timeAgo(tx.timestamp)})</span>
                </div>
                <div className="flex justify-between py-2 border-b border-border/50">
                  <span className="text-muted-foreground text-sm">Type</span>
                  <Badge variant="outline" className="text-[10px] uppercase">{tx.type}</Badge>
                </div>
              </div>
              <div className="space-y-4">
                <div className="flex justify-between py-2 border-b border-border/50">
                  <span className="text-muted-foreground text-sm">From</span>
                  <Link href={`/address/${tx.from}`} className="text-sm font-mono text-primary hover:underline truncate max-w-[200px]">
                    {tx.from}
                  </Link>
                </div>
                <div className="flex justify-between py-2 border-b border-border/50">
                  <span className="text-muted-foreground text-sm">To</span>
                  {tx.to ? (
                    <Link href={`/address/${tx.to}`} className="text-sm font-mono text-primary hover:underline truncate max-w-[200px]">
                      {tx.to}
                    </Link>
                  ) : tx.contractAddress ? (
                    <div className="flex items-center gap-1 text-sm font-mono text-primary truncate max-w-[200px]">
                      <Code className="w-3 h-3" /> [Contract Creation]
                      <Link href={`/address/${tx.contractAddress}`} className="hover:underline ml-1">
                        {formatAddress(tx.contractAddress, 6)}
                      </Link>
                    </div>
                  ) : (
                    <span className="text-muted-foreground">-</span>
                  )}
                </div>
                <div className="flex justify-between py-2 border-b border-border/50">
                  <span className="text-muted-foreground text-sm">Value</span>
                  <span className="text-sm font-mono">{formatNumber(tx.value, 6)} ZBX</span>
                </div>
                <div className="flex justify-between py-2 border-b border-border/50">
                  <span className="text-muted-foreground text-sm">Gas Price</span>
                  <span className="text-sm font-mono">{tx.gasPrice}</span>
                </div>
              </div>
            </div>
          </CardContent>
        </Card>

        {tx.input && tx.input !== "0x" && (
          <Card>
            <CardHeader className="pb-2">
              <CardTitle className="text-sm uppercase tracking-wider text-muted-foreground">Input Data</CardTitle>
            </CardHeader>
            <CardContent>
              <div className="bg-muted p-4 rounded-md overflow-x-auto">
                <code className="text-xs font-mono break-all whitespace-pre-wrap text-muted-foreground">{tx.input}</code>
              </div>
            </CardContent>
          </Card>
        )}

        {tx.logs && tx.logs.length > 0 && (
          <Card>
            <CardHeader className="pb-2">
              <CardTitle className="text-sm uppercase tracking-wider text-muted-foreground">Logs ({tx.logs.length})</CardTitle>
            </CardHeader>
            <CardContent>
              <div className="space-y-4">
                {tx.logs.map((log, i) => (
                  <div key={i} className="bg-muted/50 p-4 border border-border rounded-md">
                    <pre className="text-xs font-mono text-muted-foreground overflow-x-auto">
                      {JSON.stringify(log, null, 2)}
                    </pre>
                  </div>
                ))}
              </div>
            </CardContent>
          </Card>
        )}
      </div>
    </div>
  );
}
