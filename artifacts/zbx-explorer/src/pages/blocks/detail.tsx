import React from "react";
import { Link, useParams } from "wouter";
import { useGetBlock } from "@workspace/api-client-react";
import { formatNumber, timeAgo, formatAddress } from "@/lib/utils";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { Badge } from "@/components/ui/badge";
import { Box, ArrowLeft } from "lucide-react";

export default function BlockDetail() {
  const { blockNumber } = useParams<{ blockNumber: string }>();
  const { data: block, isLoading } = useGetBlock(Number(blockNumber), { 
    query: { enabled: !!blockNumber } 
  });

  if (isLoading) {
    return (
      <div className="space-y-6">
        <Skeleton className="h-8 w-64" />
        <Skeleton className="h-64 w-full" />
      </div>
    );
  }

  if (!block) {
    return <div>Block not found</div>;
  }

  return (
    <div className="space-y-6">
      <div className="flex items-center gap-4 mb-4">
        <Link href="/blocks" className="p-2 hover:bg-accent rounded-full transition-colors">
          <ArrowLeft className="w-5 h-5" />
        </Link>
        <div>
          <h2 className="text-2xl font-bold tracking-tight flex items-center gap-2">
            <Box className="w-6 h-6 text-primary" />
            Block <span className="font-mono text-primary">#{block.number}</span>
          </h2>
          <p className="text-muted-foreground font-mono text-xs">{block.hash}</p>
        </div>
      </div>

      <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm uppercase tracking-wider text-muted-foreground">Overview</CardTitle>
          </CardHeader>
          <CardContent className="space-y-4">
            <div className="flex justify-between py-2 border-b border-border/50">
              <span className="text-muted-foreground text-sm">Timestamp</span>
              <span className="text-sm font-medium">{new Date(block.timestamp).toLocaleString()} ({timeAgo(block.timestamp)})</span>
            </div>
            <div className="flex justify-between py-2 border-b border-border/50">
              <span className="text-muted-foreground text-sm">Transactions</span>
              <span className="text-sm font-medium">{block.txCount}</span>
            </div>
            <div className="flex justify-between py-2 border-b border-border/50">
              <span className="text-muted-foreground text-sm">Proposer</span>
              <Link href={`/validators/${block.proposer}`} className="text-sm font-mono text-primary hover:underline">
                {formatAddress(block.proposer, 12)}
              </Link>
            </div>
            <div className="flex justify-between py-2 border-b border-border/50">
              <span className="text-muted-foreground text-sm">Size</span>
              <span className="text-sm font-medium">{formatNumber(block.size, 0)} bytes</span>
            </div>
          </CardContent>
        </Card>

        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm uppercase tracking-wider text-muted-foreground">Gas & State</CardTitle>
          </CardHeader>
          <CardContent className="space-y-4">
            <div className="flex justify-between py-2 border-b border-border/50">
              <span className="text-muted-foreground text-sm">Gas Used</span>
              <span className="text-sm font-mono">{formatNumber(block.gasUsed, 0)} <span className="text-muted-foreground">({((parseFloat(block.gasUsed) / parseFloat(block.gasLimit)) * 100).toFixed(2)}%)</span></span>
            </div>
            <div className="flex justify-between py-2 border-b border-border/50">
              <span className="text-muted-foreground text-sm">Gas Limit</span>
              <span className="text-sm font-mono">{formatNumber(block.gasLimit, 0)}</span>
            </div>
            <div className="flex justify-between py-2 border-b border-border/50">
              <span className="text-muted-foreground text-sm">Base Fee</span>
              <span className="text-sm font-mono">{block.baseFeePerGas ? `${formatNumber(block.baseFeePerGas, 4)} gwei` : '-'}</span>
            </div>
            <div className="flex justify-between py-2 border-b border-border/50">
              <span className="text-muted-foreground text-sm">Parent Hash</span>
              <Link href={`/blocks/${block.number - 1}`} className="text-sm font-mono text-primary hover:underline truncate max-w-[200px]">
                {block.parentHash}
              </Link>
            </div>
          </CardContent>
        </Card>
      </div>

      <Card>
        <CardHeader>
          <CardTitle>Transactions</CardTitle>
        </CardHeader>
        <CardContent className="p-0">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Hash</TableHead>
                <TableHead>Type</TableHead>
                <TableHead>From / To</TableHead>
                <TableHead>Status</TableHead>
                <TableHead className="text-right">Value</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {block.transactions?.map((tx) => (
                <TableRow key={tx.hash}>
                  <TableCell className="font-mono text-xs">
                    <Link href={`/txs/${tx.hash}`} className="text-primary hover:underline">
                      {formatAddress(tx.hash, 8)}
                    </Link>
                  </TableCell>
                  <TableCell>
                    <Badge variant="outline" className="text-[10px] uppercase">
                      {tx.type}
                    </Badge>
                  </TableCell>
                  <TableCell className="font-mono text-xs text-muted-foreground">
                    <div className="flex flex-col gap-1">
                      <span className="flex gap-2">
                        <span className="w-4 text-center">F</span>
                        <Link href={`/address/${tx.from}`} className="hover:text-primary transition-colors">{formatAddress(tx.from, 6)}</Link>
                      </span>
                      {tx.to && (
                        <span className="flex gap-2">
                          <span className="w-4 text-center">T</span>
                          <Link href={`/address/${tx.to}`} className="hover:text-primary transition-colors">{formatAddress(tx.to, 6)}</Link>
                        </span>
                      )}
                    </div>
                  </TableCell>
                  <TableCell>
                    <Badge variant={tx.status === 'success' ? 'default' : tx.status === 'failed' ? 'destructive' : 'secondary'} className="text-[10px]">
                      {tx.status}
                    </Badge>
                  </TableCell>
                  <TableCell className="text-right font-mono text-xs">
                    {formatNumber(tx.value, 4)} ZBX
                  </TableCell>
                </TableRow>
              ))}
              {(!block.transactions || block.transactions.length === 0) && (
                <TableRow>
                  <TableCell colSpan={5} className="text-center py-8 text-muted-foreground">
                    No transactions in this block.
                  </TableCell>
                </TableRow>
              )}
            </TableBody>
          </Table>
        </CardContent>
      </Card>
    </div>
  );
}
